//! Terminal frontend for the interactive REPL (ADR-0074).
//!
//! The REPL keeps writing plain terminal text and reading lines. This layer interprets that text
//! the way a terminal would: every finished line goes to the scrollback above a pane pinned to the
//! bottom of the screen, and the unfinished tail (a prompt, or the live progress line the REPL keeps
//! rewriting with `\r`) is shown inside the pane. The pane holds an input box that stays editable
//! while a Run is in flight, so the next goal can be typed and queued, and Ctrl+C cancels the Run.
//!
//! Only this thread writes to the terminal, so scrollback output and pane redraws never interleave.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::{cursor, queue, terminal};
use unicode_width::UnicodeWidthChar as _;

use crate::repl::{Cancellation, InputMessage, InterruptibleInput, MAX_REPL_LINE_BYTES};

const MAX_HISTORY_ENTRIES: usize = 500;
/// Input rows the box shows before it scrolls with the caret.
const MAX_INPUT_ROWS: usize = 8;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

/// Prompts the REPL writes before reading a goal or the next line of one.
const GOAL_PROMPT: &str = "xgen> ";
const CONTINUATION_PROMPT: &str = "...> ";

enum Message {
    Output(Vec<u8>),
    Request,
    Shutdown,
}

/// The REPL's output: bytes are handed to the frontend thread in write order.
pub(crate) struct TuiOutput {
    sender: Sender<Message>,
}

impl Write for TuiOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sender
            .send(Message::Output(bytes.to_vec()))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Restores the terminal when the REPL ends, including on an early return.
pub(crate) struct TuiGuard {
    sender: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        restore_terminal();
    }
}

fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = queue!(stdout, DisableBracketedPaste, cursor::Show);
    let _ = stdout.flush();
    let _ = terminal::disable_raw_mode();
}

/// Start the frontend on the current terminal. `footer` names the session (directory, mode).
pub(crate) fn start(
    cancellation: Cancellation,
    history: Option<PathBuf>,
    footer: String,
) -> io::Result<(InterruptibleInput, TuiOutput, TuiGuard)> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = queue!(stdout, EnableBracketedPaste).and_then(|()| stdout.flush()) {
        restore_terminal();
        return Err(error);
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));

    let (sender, receiver) = mpsc::channel();
    let (lines, input) = mpsc::sync_channel(1);
    let frontend_cancellation = cancellation.clone();
    let thread = thread::spawn(move || {
        let mut frontend = Frontend::new(frontend_cancellation, history, footer, lines);
        frontend.run(&receiver);
    });
    let request_sender = sender.clone();
    let input = InterruptibleInput::from_frontend(
        input,
        Box::new(move || request_sender.send(Message::Request).is_ok()),
        cancellation,
    );
    Ok((
        input,
        TuiOutput {
            sender: sender.clone(),
        },
        TuiGuard {
            sender,
            thread: Some(thread),
        },
    ))
}

struct Frontend {
    cancellation: Cancellation,
    lines: mpsc::SyncSender<InputMessage>,
    history_path: Option<PathBuf>,
    footer: String,
    screen: ScreenText,
    editor: LineEditor,
    /// The REPL is waiting for a line.
    requested: bool,
    /// Goals typed while a Run was in flight, sent at the next goal prompt.
    queue: VecDeque<String>,
    /// The remaining lines of a multi-line submission, answered at `...>` prompts.
    continuation: VecDeque<String>,
    cancelling: bool,
    started: Instant,
    drawn: DrawnPane,
}

#[derive(Default)]
struct DrawnPane {
    /// Row of the cursor inside the pane, counted from the pane's first row.
    caret_row: u16,
}

impl Frontend {
    fn new(
        cancellation: Cancellation,
        history_path: Option<PathBuf>,
        footer: String,
        lines: mpsc::SyncSender<InputMessage>,
    ) -> Self {
        let history = history_path
            .as_deref()
            .map(load_history)
            .unwrap_or_default();
        Self {
            cancellation,
            lines,
            history_path,
            footer,
            screen: ScreenText::default(),
            editor: LineEditor::with_history(history),
            requested: false,
            queue: VecDeque::new(),
            continuation: VecDeque::new(),
            cancelling: false,
            started: Instant::now(),
            drawn: DrawnPane::default(),
        }
    }

    fn run(&mut self, receiver: &Receiver<Message>) {
        let mut finished = Vec::new();
        let mut last_draw = Instant::now();
        loop {
            // Drain everything the REPL produced before looking at the keyboard.
            loop {
                match receiver.try_recv() {
                    Ok(Message::Output(bytes)) => finished.extend(self.screen.push(&bytes)),
                    Ok(Message::Request) => self.on_request(&mut finished),
                    Ok(Message::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = self.draw(&finished, false);
                        let _ = clear_pane(self.drawn.caret_row);
                        return;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let mut dirty = !finished.is_empty();
            let event = event::poll(Duration::from_millis(25))
                .and_then(|ready| ready.then(event::read).transpose());
            let Ok(event) = event else {
                let _ = self.lines.send(InputMessage::Unavailable);
                return;
            };
            if let Some(event) = event {
                dirty |= self.on_event(event, &mut finished);
            }
            if dirty || last_draw.elapsed() >= REDRAW_INTERVAL {
                if self.draw(&finished, true).is_err() {
                    let _ = self.lines.send(InputMessage::Unavailable);
                    return;
                }
                finished.clear();
                last_draw = Instant::now();
            }
            // A cancelled Run is over once the REPL asks for input again.
            if self.requested {
                self.cancelling = false;
            }
        }
    }

    fn prompt(&self) -> String {
        strip_controls(self.screen.tail())
    }

    fn on_request(&mut self, finished: &mut Vec<Vec<u8>>) {
        self.requested = true;
        let prompt = self.prompt();
        if prompt == CONTINUATION_PROMPT
            && let Some(line) = self.continuation.pop_front()
        {
            self.screen.clear_tail();
            self.send_line(line);
            return;
        }
        self.continuation.clear();
        if prompt == GOAL_PROMPT
            && let Some(goal) = self.queue.pop_front()
        {
            self.submit(&goal, finished);
        }
    }

    /// Returns whether the pane must be redrawn.
    fn on_event(&mut self, event: Event, finished: &mut Vec<Vec<u8>>) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, finished),
            Event::Paste(text) => {
                self.editor
                    .insert_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
                true
            }
            Event::Resize(..) => true,
            _ => false,
        }
    }

    fn on_key(&mut self, key: KeyEvent, finished: &mut Vec<Vec<u8>>) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('c') if control => self.interrupt(),
            KeyCode::Char('d') if control => {
                if self.requested && self.editor.is_empty() {
                    self.requested = false;
                    self.screen.clear_tail();
                    let _ = self.lines.send(InputMessage::EndOfInput);
                } else {
                    self.editor.delete();
                }
            }
            KeyCode::Enter if alt || shift => self.editor.insert('\n'),
            KeyCode::Char('j') if control => self.editor.insert('\n'),
            KeyCode::Enter => {
                let text = self.editor.take();
                if self.requested {
                    self.submit(&text, finished);
                } else if !text.trim().is_empty() {
                    self.queue.push_back(text);
                }
            }
            KeyCode::Char('a') if control => self.editor.home(),
            KeyCode::Char('e') if control => self.editor.end(),
            KeyCode::Char('u') if control => self.editor.kill_to_start(),
            KeyCode::Char('k') if control => self.editor.kill_to_end(),
            KeyCode::Char('w') if control => self.editor.delete_word(),
            KeyCode::Char(character) if !control => self.editor.insert(character),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Up => self.editor.previous(),
            KeyCode::Down => self.editor.next(),
            KeyCode::Esc if !self.requested => {
                self.queue.clear();
            }
            _ => return false,
        }
        true
    }

    fn interrupt(&mut self) {
        if !self.requested {
            // A Run is in flight: ask the driver to stop at its next boundary.
            self.cancellation.request();
            self.cancelling = true;
            self.queue.clear();
        } else if !self.editor.is_empty() {
            self.editor.take();
        } else {
            self.requested = false;
            self.screen.clear_tail();
            let _ = self.lines.send(InputMessage::Interrupted);
        }
    }

    fn submit(&mut self, text: &str, finished: &mut Vec<Vec<u8>>) {
        let prompt = self.prompt();
        self.screen.clear_tail();
        finished.push(echo_line(&prompt, text));
        let goal = prompt == GOAL_PROMPT || prompt == CONTINUATION_PROMPT;
        if goal && !text.trim().is_empty() {
            self.editor.remember(text);
            if let Some(path) = &self.history_path {
                append_history(path, text);
            }
        }
        // The REPL joins lines ending in `\` into one goal.
        let mut lines = text.split('\n').map(str::to_owned).collect::<VecDeque<_>>();
        let first = lines.pop_front().unwrap_or_default();
        if goal && !lines.is_empty() {
            let last = lines.len() - 1;
            self.continuation = lines
                .into_iter()
                .enumerate()
                .map(|(index, line)| if index == last { line } else { line + "\\" })
                .collect();
            self.send_line(first + "\\");
        } else {
            self.send_line(first);
        }
    }

    fn send_line(&mut self, line: String) {
        self.requested = false;
        let message = if line.len() > MAX_REPL_LINE_BYTES {
            InputMessage::TooLarge
        } else {
            InputMessage::Line(line)
        };
        let _ = self.lines.send(message);
    }

    fn draw(&mut self, finished: &[Vec<u8>], with_pane: bool) -> io::Result<()> {
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let mut stdout = io::stdout().lock();
        // `CSI 0 A` moves one row on most terminals, so never send a zero move.
        if self.drawn.caret_row > 0 {
            queue!(stdout, cursor::MoveUp(self.drawn.caret_row))?;
        }
        queue!(
            stdout,
            cursor::MoveToColumn(0),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        for line in finished {
            stdout.write_all(line)?;
            stdout.write_all(b"\x1b[0m\r\n")?;
        }
        if !with_pane {
            self.drawn = DrawnPane::default();
            return stdout.flush();
        }
        let view = PaneView {
            width: usize::from(width),
            height: usize::from(height),
            prompt: self.prompt(),
            requested: self.requested,
            cancelling: self.cancelling,
            queue: &self.queue,
            footer: &self.footer,
            spinner: SPINNER[usize::try_from(self.started.elapsed().as_millis() / 250)
                .unwrap_or(0)
                % SPINNER.len()],
        };
        let pane = layout(&view, &self.editor);
        let last_row = pane.rows.len().saturating_sub(1);
        stdout.write_all(pane.rows.join("\r\n").as_bytes())?;
        let up = u16::try_from(last_row.saturating_sub(pane.caret_row)).unwrap_or(0);
        if up > 0 {
            queue!(stdout, cursor::MoveUp(up))?;
        }
        queue!(
            stdout,
            cursor::MoveToColumn(u16::try_from(pane.caret_column).unwrap_or(0))
        )?;
        self.drawn.caret_row = u16::try_from(pane.caret_row).unwrap_or(0);
        stdout.flush()
    }
}

fn clear_pane(caret_row: u16) -> io::Result<()> {
    let mut stdout = io::stdout();
    if caret_row > 0 {
        queue!(stdout, cursor::MoveUp(caret_row))?;
    }
    queue!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )?;
    stdout.flush()
}

/// How a submitted line is kept in the scrollback.
fn echo_line(prompt: &str, text: &str) -> Vec<u8> {
    let mut line = String::new();
    if prompt == GOAL_PROMPT || prompt == CONTINUATION_PROMPT {
        let _ = write!(line, "\x1b[1m> \x1b[0m");
        line.push_str(&text.replace('\n', "\r\n  "));
    } else {
        line.push_str(prompt);
        line.push_str(text);
    }
    line.into_bytes()
}

/// The REPL's text split the way a terminal shows it: finished lines and the line still open.
#[derive(Default)]
struct ScreenText {
    tail: Vec<u8>,
}

impl ScreenText {
    /// Feed bytes and return the lines they finished, each without its line break.
    fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut finished = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.tail);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                finished.push(visible_after_carriage_return(line));
            } else {
                self.tail.push(byte);
            }
        }
        finished
    }

    /// What is written after the last line break, as the terminal would show it after any `\r`.
    fn tail(&self) -> &[u8] {
        let start = self
            .tail
            .iter()
            .rposition(|&byte| byte == b'\r')
            .map_or(0, |index| index + 1);
        &self.tail[start..]
    }

    fn clear_tail(&mut self) {
        self.tail.clear();
    }
}

fn visible_after_carriage_return(line: Vec<u8>) -> Vec<u8> {
    match line.iter().rposition(|&byte| byte == b'\r') {
        Some(index) => line[index + 1..].to_vec(),
        None => line,
    }
}

/// The text without escape sequences or control characters, for measuring and for the pane.
fn strip_controls(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut stripped = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            if characters.next() == Some('[') {
                for parameter in characters.by_ref() {
                    if ('@'..='~').contains(&parameter) {
                        break;
                    }
                }
            }
        } else if !character.is_control() {
            stripped.push(character);
        }
    }
    stripped
}

/// A line buffer with a caret, word and line kills, and history recall.
#[derive(Default)]
struct LineEditor {
    text: String,
    /// Byte offset of the caret, always on a character boundary.
    caret: usize,
    history: Vec<String>,
    /// Position while recalling history; `history.len()` means the draft.
    recall: usize,
    draft: String,
}

impl LineEditor {
    fn with_history(history: Vec<String>) -> Self {
        let recall = history.len();
        Self {
            history,
            recall,
            ..Self::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn insert(&mut self, character: char) {
        self.text.insert(self.caret, character);
        self.caret += character.len_utf8();
    }

    fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.caret, text);
        self.caret += text.len();
    }

    fn take(&mut self) -> String {
        self.caret = 0;
        self.recall = self.history.len();
        self.draft.clear();
        std::mem::take(&mut self.text)
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |character| self.caret + character.len_utf8())
    }

    fn backspace(&mut self) {
        let start = self.previous_boundary();
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
    }

    fn delete(&mut self) {
        let end = self.next_boundary();
        self.text.replace_range(self.caret..end, "");
    }

    fn left(&mut self) {
        self.caret = self.previous_boundary();
    }

    fn right(&mut self) {
        self.caret = self.next_boundary();
    }

    fn line_start(&self) -> usize {
        self.text[..self.caret]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.caret..]
            .find('\n')
            .map_or(self.text.len(), |index| self.caret + index)
    }

    fn home(&mut self) {
        self.caret = self.line_start();
    }

    fn end(&mut self) {
        self.caret = self.line_end();
    }

    fn kill_to_start(&mut self) {
        let start = self.line_start();
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
    }

    fn kill_to_end(&mut self) {
        let end = self.line_end();
        self.text.replace_range(self.caret..end, "");
    }

    fn delete_word(&mut self) {
        let before = &self.text[..self.caret];
        let trimmed = before.trim_end_matches(|character: char| character.is_whitespace());
        let start = trimmed
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map_or(0, |(index, character)| index + character.len_utf8());
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
    }

    /// Up: move to the line above, or recall an older entry from the first line.
    fn previous(&mut self) {
        if self.line_start() > 0 {
            self.move_vertically(false);
            return;
        }
        if self.recall == 0 {
            return;
        }
        if self.recall == self.history.len() {
            self.draft = self.text.clone();
        }
        self.recall -= 1;
        self.text = self.history[self.recall].clone();
        self.caret = self.text.len();
    }

    /// Down: move to the line below, or a newer entry from the last line.
    fn next(&mut self) {
        if self.line_end() < self.text.len() {
            self.move_vertically(true);
            return;
        }
        if self.recall >= self.history.len() {
            return;
        }
        self.recall += 1;
        self.text = if self.recall == self.history.len() {
            std::mem::take(&mut self.draft)
        } else {
            self.history[self.recall].clone()
        };
        self.caret = self.text.len();
    }

    fn move_vertically(&mut self, down: bool) {
        let column = self.text[self.line_start()..self.caret].chars().count();
        let target_start = if down {
            self.line_end() + 1
        } else {
            let previous_end = self.line_start() - 1;
            self.text[..previous_end]
                .rfind('\n')
                .map_or(0, |index| index + 1)
        };
        let target_end = self.text[target_start..]
            .find('\n')
            .map_or(self.text.len(), |index| target_start + index);
        self.caret = self.text[target_start..target_end]
            .char_indices()
            .nth(column)
            .map_or(target_end, |(index, _)| target_start + index);
    }

    fn remember(&mut self, entry: &str) {
        if self.history.last().map(String::as_str) != Some(entry) {
            self.history.push(entry.to_owned());
            if self.history.len() > MAX_HISTORY_ENTRIES {
                self.history.remove(0);
            }
        }
        self.recall = self.history.len();
    }
}

struct PaneView<'a> {
    width: usize,
    height: usize,
    prompt: String,
    requested: bool,
    cancelling: bool,
    queue: &'a VecDeque<String>,
    footer: &'a str,
    spinner: char,
}

struct Pane {
    rows: Vec<String>,
    caret_row: usize,
    caret_column: usize,
}

/// The pinned pane: status, queued goals, the input box, and the footer.
fn layout(view: &PaneView<'_>, editor: &LineEditor) -> Pane {
    let width = view.width.max(20);
    let inner = width - 4;
    let mut rows = Vec::new();
    let goal_prompt = view.prompt == GOAL_PROMPT || view.prompt == CONTINUATION_PROMPT;
    if view.cancelling {
        rows.push(format!(
            "\x1b[33m{} cancelling at the next step…\x1b[0m",
            view.spinner
        ));
    } else if !view.requested && !view.prompt.is_empty() {
        rows.push(format!(
            "\x1b[2m{} {}\x1b[0m",
            view.spinner,
            fit(&view.prompt, inner)
        ));
    } else if view.requested && !goal_prompt && !view.prompt.is_empty() {
        rows.push(format!("\x1b[1;33m{}\x1b[0m", fit(&view.prompt, width - 1)));
    }
    for goal in view.queue {
        rows.push(format!(
            "\x1b[2mqueued: {}\x1b[0m",
            fit(&goal.replace('\n', " "), inner.saturating_sub(8))
        ));
    }
    rows.push(format!("╭{}╮", "─".repeat(width - 2)));
    let (lines, caret_line, caret_column) = wrap_input(&editor.text, editor.caret, inner - 2);
    let visible = MAX_INPUT_ROWS.min(view.height.saturating_sub(rows.len() + 3).max(1));
    let first = caret_line.saturating_sub(visible - 1);
    let box_top = rows.len();
    for (index, line) in lines.iter().enumerate().skip(first).take(visible) {
        let marker = if index == 0 { "> " } else { "  " };
        let pad = (inner - 2).saturating_sub(display_width(line));
        rows.push(format!("│ {marker}{line}{} │", " ".repeat(pad)));
    }
    rows.push(format!("╰{}╯", "─".repeat(width - 2)));
    let hint = if view.requested {
        "Enter send · Alt+Enter newline · Ctrl+C clear · Ctrl+D exit"
    } else {
        "Enter queue · Esc drop queue · Ctrl+C cancel run"
    };
    rows.push(format!(
        "\x1b[2m{}\x1b[0m",
        fit(&format!("{}  {hint}", view.footer), width - 1)
    ));
    Pane {
        caret_row: box_top + caret_line - first,
        caret_column: 4 + caret_column,
        rows,
    }
}

/// Wrap the input to `width` columns; return the rows and the caret's row and column.
fn wrap_input(text: &str, caret: usize, width: usize) -> (Vec<String>, usize, usize) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut column = 0;
    let mut caret_position = (0, 0);
    for (index, character) in text.char_indices() {
        if character == '\n' {
            if index == caret {
                caret_position = (rows.len() - 1, column);
            }
            rows.push(String::new());
            column = 0;
            continue;
        }
        let character_width = character.width().unwrap_or(0);
        if column + character_width > width {
            rows.push(String::new());
            column = 0;
        }
        if index == caret {
            caret_position = (rows.len() - 1, column);
        }
        if let Some(row) = rows.last_mut() {
            row.push(character);
        }
        column += character_width;
    }
    if caret >= text.len() {
        if column >= width {
            rows.push(String::new());
            column = 0;
        }
        caret_position = (rows.len() - 1, column);
    }
    (rows, caret_position.0, caret_position.1)
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

/// Cut `text` to `width` columns, marking the cut.
fn fit(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let mut fitted = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width + 1 > width {
            break;
        }
        fitted.push(character);
        used += character_width;
    }
    fitted.push('…');
    fitted
}

/// One JSON string per line, so multi-line goals survive a reload.
fn load_history(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries = text
        .lines()
        .filter_map(|line| serde_json::from_str::<String>(line).ok())
        .collect::<Vec<_>>();
    let excess = entries.len().saturating_sub(MAX_HISTORY_ENTRIES);
    entries.drain(..excess);
    entries
}

fn append_history(path: &Path, entry: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(encoded) = serde_json::to_string(entry) else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{encoded}");
    keep_private(path);
}

/// Goals can name private paths or hosts, so only the user may read the history file.
fn keep_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_lines_scroll_and_the_open_line_stays_in_the_pane() {
        let mut screen = ScreenText::default();
        assert!(screen.push(b"Thinking\xe2\x80\xa6 \xc2\xb7 0s").is_empty());
        assert_eq!(strip_controls(screen.tail()), "Thinking… · 0s");
        assert!(
            screen
                .push(b"\r\x1b[2KReading\xe2\x80\xa6 \xc2\xb7 1s")
                .is_empty()
        );
        assert_eq!(strip_controls(screen.tail()), "Reading… · 1s");
        let finished = screen.push(b"\r\x1b[2K  read README.md\nanswer\r\nxgen> ");
        assert_eq!(
            finished
                .iter()
                .map(|line| strip_controls(line))
                .collect::<Vec<_>>(),
            ["  read README.md", "answer"]
        );
        assert_eq!(strip_controls(screen.tail()), GOAL_PROMPT);
    }

    #[test]
    fn the_editor_moves_by_characters_words_lines_and_history() {
        let mut editor = LineEditor::with_history(vec!["older".to_owned(), "newer".to_owned()]);
        for character in "한글 word".chars() {
            editor.insert(character);
        }
        editor.left();
        editor.backspace();
        assert_eq!(editor.text, "한글 wod");
        editor.delete_word();
        assert_eq!(editor.text, "한글 d");
        editor.home();
        editor.delete();
        assert_eq!(editor.text, "글 d");
        editor.end();
        editor.insert('\n');
        editor.insert_str("second");
        editor.previous();
        assert_eq!(&editor.text[..editor.caret], "글 d");
        editor.previous();
        assert_eq!(editor.text, "newer");
        editor.previous();
        assert_eq!(editor.text, "older");
        editor.next();
        editor.next();
        assert_eq!(editor.text, "글 d\nsecond");
        assert_eq!(editor.take(), "글 d\nsecond");
        assert!(editor.is_empty());
    }

    #[test]
    fn the_input_wraps_by_display_width_and_tracks_the_caret() {
        let (rows, row, column) = wrap_input("abcdef한글", "abcdef한".len(), 4);
        assert_eq!(rows, ["abcd", "ef한", "글"]);
        assert_eq!((row, column), (2, 0));
        let (rows, row, column) = wrap_input("ab\ncd", 5, 10);
        assert_eq!(rows, ["ab", "cd"]);
        assert_eq!((row, column), (1, 2));
    }

    #[test]
    fn the_pane_shows_the_run_status_or_the_pending_question() {
        let queue = VecDeque::from(["next goal".to_owned()]);
        let mut editor = LineEditor::default();
        editor.insert_str("typed");
        let running = PaneView {
            width: 80,
            height: 20,
            prompt: "Reading… · 3s".to_owned(),
            requested: false,
            cancelling: false,
            queue: &queue,
            footer: "~/work · trust",
            spinner: '|',
        };
        let pane = layout(&running, &editor);
        let text = pane
            .rows
            .iter()
            .map(|row| strip_controls(row.as_bytes()))
            .collect::<Vec<_>>();
        assert_eq!(text[0], "| Reading… · 3s");
        assert_eq!(text[1], "queued: next goal");
        assert!(text[3].starts_with("│ > typed"));
        assert!(text.iter().all(|row| display_width(row) <= 80), "{text:?}");
        assert_eq!((pane.caret_row, pane.caret_column), (3, 9));
        assert!(text[5].contains("Ctrl+C cancel run"));

        let empty = VecDeque::new();
        let asking = PaneView {
            prompt: "Allow read for this durable continuation? [y/a/N] ".to_owned(),
            requested: true,
            queue: &empty,
            ..running
        };
        let text = layout(&asking, &LineEditor::default())
            .rows
            .iter()
            .map(|row| strip_controls(row.as_bytes()))
            .collect::<Vec<_>>();
        assert!(text[0].starts_with("Allow read"));
        assert!(text.last().unwrap().contains("Ctrl+D exit"));
    }

    #[test]
    fn history_round_trips_multi_line_goals_privately() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/repl-history.jsonl");
        append_history(&path, "first");
        append_history(&path, "two\nlines");
        assert_eq!(load_history(&path), ["first", "two\nlines"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
