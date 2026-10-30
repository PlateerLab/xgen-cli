//! Bounded result previews and the read-only result viewer. No Run or model state lives here.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::repl::ResultKind;
use crate::tui::{fit, strip_controls, wrap_input};

const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESULTS: usize = 20;
const FOLD_AFTER_ROWS: usize = 12;
const PREVIEW_HEAD: usize = 4;
const PREVIEW_TAIL: usize = 2;

struct SavedResult {
    id: u64,
    kind: ResultKind,
    lines: Vec<Vec<u8>>,
    bytes: usize,
}

pub(crate) struct ResultHistory {
    results: VecDeque<SavedResult>,
    bytes: usize,
    next_id: u64,
    max_bytes: usize,
    max_results: usize,
}

impl Default for ResultHistory {
    fn default() -> Self {
        Self {
            results: VecDeque::new(),
            bytes: 0,
            next_id: 1,
            max_bytes: MAX_RESULT_BYTES,
            max_results: MAX_RESULTS,
        }
    }
}

impl ResultHistory {
    /// Never fold a result that cannot be retained in full.
    pub(crate) fn capture(
        &mut self,
        kind: ResultKind,
        lines: Vec<Vec<u8>>,
        width: usize,
    ) -> Vec<Vec<u8>> {
        let bytes = lines
            .iter()
            .map(Vec::capacity)
            .sum::<usize>()
            .saturating_add(
                lines
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Vec<u8>>()),
            );
        if lines.is_empty() || bytes > self.max_bytes || self.max_results == 0 {
            return lines;
        }
        while self.bytes + bytes > self.max_bytes || self.results.len() >= self.max_results {
            if let Some(old) = self.results.pop_front() {
                self.bytes -= old.bytes;
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let width = width.max(1);
        let wrapped = wrap_lines(&lines, width);
        let preview = if wrapped.len() > FOLD_AFTER_ROWS {
            let mut preview: Vec<Vec<u8>> = wrapped
                .iter()
                .take(PREVIEW_HEAD)
                .map(|row| row.as_bytes().to_vec())
                .collect();
            preview.push(
                fit(
                    &format!(
                        "Ctrl+O: {} #{id} · {} rows hidden",
                        kind.label(),
                        wrapped.len().saturating_sub(PREVIEW_HEAD + PREVIEW_TAIL)
                    ),
                    width,
                )
                .into_bytes(),
            );
            preview.extend(
                wrapped
                    .iter()
                    .skip(wrapped.len().saturating_sub(PREVIEW_TAIL))
                    .map(|row| row.as_bytes().to_vec()),
            );
            preview
        } else {
            lines.clone()
        };
        self.bytes += bytes;
        self.results.push_back(SavedResult {
            id,
            kind,
            lines,
            bytes,
        });
        preview
    }

    pub(crate) fn latest(&self) -> Option<u64> {
        self.results.back().map(|result| result.id)
    }

    pub(crate) fn contains(&self, id: u64) -> bool {
        self.results.iter().any(|result| result.id == id)
    }
}

fn wrap_lines(lines: &[Vec<u8>], width: usize) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| {
            let plain = strip_controls(line);
            // A caret at the beginning avoids an extra empty row on an exact-width final line.
            wrap_input(&plain, 0, width)
                .0
                .into_iter()
                .map(|row| fit(&row, width))
        })
        .collect()
}

pub(crate) enum ViewerAction {
    Close,
    Redraw,
    Ignore,
}

pub(crate) struct ResultViewer {
    pub(crate) id: u64,
    offset: usize,
    cached_width: usize,
    rows: Vec<String>,
    page_rows: usize,
}

impl ResultViewer {
    pub(crate) const fn new(id: u64) -> Self {
        Self {
            id,
            offset: 0,
            cached_width: 0,
            rows: Vec::new(),
            page_rows: 1,
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent, history: &ResultHistory) -> ViewerAction {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('o' | 'c' | 'd'))
        {
            return ViewerAction::Close;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ViewerAction::Close,
            KeyCode::Up | KeyCode::Char('k') => self.offset = self.offset.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.offset = self.offset.saturating_add(1),
            KeyCode::PageUp => self.offset = self.offset.saturating_sub(self.page_rows),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.offset = self.offset.saturating_add(self.page_rows);
            }
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = usize::MAX,
            KeyCode::Left | KeyCode::Right => {
                let current = history
                    .results
                    .iter()
                    .position(|result| result.id == self.id)
                    .unwrap_or(0);
                let selected = if key.code == KeyCode::Left {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(history.results.len().saturating_sub(1))
                };
                if let Some(result) = history.results.get(selected) {
                    self.id = result.id;
                    self.cached_width = 0;
                    self.offset = 0;
                }
            }
            _ => return ViewerAction::Ignore,
        }
        ViewerAction::Redraw
    }

    /// Plain sanitized text only; the terminal layer owns alternate-screen transitions.
    pub(crate) fn frame(
        &mut self,
        history: &ResultHistory,
        width: usize,
        height: usize,
        notice: &str,
    ) -> Vec<String> {
        let width = width.max(1);
        let height = height.max(1);
        let Some((index, result)) = history
            .results
            .iter()
            .enumerate()
            .find(|(_, result)| result.id == self.id)
        else {
            return Vec::new();
        };
        if self.cached_width != width {
            self.rows = wrap_lines(&result.lines, width);
            self.cached_width = width;
        }
        self.page_rows = height.saturating_sub(2).max(1);
        self.offset = self
            .offset
            .min(self.rows.len().saturating_sub(self.page_rows));
        if height == 1 {
            return vec![self.rows.get(self.offset).cloned().unwrap_or_default()];
        }
        let mut frame = vec![fit(
            &format!(
                "{} #{} · {}/{} results · row {}/{}",
                result.kind.label(),
                result.id,
                index + 1,
                history.results.len(),
                self.offset + 1,
                self.rows.len()
            ),
            width,
        )];
        frame.extend(
            self.rows
                .iter()
                .skip(self.offset)
                .take(height.saturating_sub(2))
                .cloned(),
        );
        while frame.len() < height - 1 {
            frame.push(String::new());
        }
        let footer = if notice.is_empty() {
            "Esc return · Up/Down PgUp/PgDn · Left/Right result"
        } else {
            notice
        };
        frame.push(fit(footer, width));
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_changes_fold_without_discarding_middle_or_final_rows() {
        for kind in [ResultKind::Answer, ResultKind::Changes] {
            let mut history = ResultHistory::default();
            let lines: Vec<_> = (0..40)
                .map(|n| format!("row-{n:02}").into_bytes())
                .collect();
            let preview = history.capture(kind, lines.clone(), 80);
            assert_eq!(preview.len(), PREVIEW_HEAD + PREVIEW_TAIL + 1);
            assert!(String::from_utf8_lossy(&preview[PREVIEW_HEAD]).contains("Ctrl+O"));
            assert_eq!(preview.last().unwrap(), b"row-39");
            assert_eq!(history.results.back().unwrap().lines, lines);
            let mut viewer = ResultViewer::new(history.latest().unwrap());
            assert!(
                viewer
                    .frame(&history, 80, 10, "")
                    .iter()
                    .any(|row| row == "row-00")
            );
            viewer.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE), &history);
            assert!(
                viewer
                    .frame(&history, 80, 10, "")
                    .iter()
                    .any(|row| row == "row-39")
            );
        }
    }

    #[test]
    fn wrapping_is_folded_and_oversized_results_are_shown_in_full() {
        let mut history = ResultHistory::default();
        let text = vec!["한글".repeat(200).into_bytes()];
        assert!(history.capture(ResultKind::Answer, text.clone(), 20).len() < 12);
        let mut history = ResultHistory {
            max_bytes: 4,
            ..ResultHistory::default()
        };
        assert_eq!(history.capture(ResultKind::Answer, text.clone(), 20), text);
        assert!(history.latest().is_none());
    }

    #[test]
    fn viewer_resizes_navigates_and_keeps_every_frame_within_the_terminal() {
        let mut history = ResultHistory::default();
        history.capture(
            ResultKind::Answer,
            vec!["한글 long paragraph ".repeat(40).into_bytes()],
            80,
        );
        history.capture(ResultKind::Changes, vec![b"second result".to_vec()], 80);
        let mut viewer = ResultViewer::new(history.latest().unwrap());
        viewer.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &history);
        let first = viewer.id;
        for width in 1..=80 {
            for height in 1..=24 {
                let frame = viewer.frame(&history, width, height, "Approval pending · Esc return");
                assert_eq!(frame.len(), height);
                assert!(
                    frame
                        .iter()
                        .all(|row| crate::tui::display_width(row) <= width)
                );
                assert_eq!(viewer.id, first);
            }
        }
        assert!(matches!(
            viewer.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &history),
            ViewerAction::Ignore
        ));
        assert!(matches!(
            viewer.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &history),
            ViewerAction::Close
        ));
    }

    #[test]
    fn retention_is_bounded_and_short_results_keep_their_original_styling() {
        let mut history = ResultHistory {
            max_results: 2,
            max_bytes: 128,
            ..ResultHistory::default()
        };
        let styled = vec![b"\x1b[1mshort\x1b[0m".to_vec()];
        assert_eq!(
            history.capture(ResultKind::Answer, styled.clone(), 80),
            styled
        );
        let first = history.latest().unwrap();
        for _ in 0..3 {
            history.capture(ResultKind::Answer, vec![b"later".to_vec()], 80);
        }
        assert!(!history.contains(first));
        assert!(history.results.len() <= 2 && history.bytes <= 128);
    }
}
