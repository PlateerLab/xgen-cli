//! Independent background Runs in the interactive session (ADR-0094).
//!
//! `/bg GOAL` starts a separate durable Run on its own thread while the foreground Run and the
//! input box keep working. A background Run never asks: it uses only the approvals the session
//! already granted, and stops (paused) at the first boundary that would need a new one, so the
//! user can `/resume` it in the foreground. The runtime needs nothing new: leases are per Run and
//! digest-checked writes keep two Runs from silently overwriting the same file.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use xgen_cli::{DriverProgress, DriverProgressControl, LocalCommandResult, RejectionReason};

use crate::repl::{InvocationGrants, ReplFailure};

/// Background Runs allowed to execute at once in one session.
pub(crate) const MAX_RUNNING: usize = 3;
/// Finished entries kept for `/runs` and `/show`; the oldest finished one is forgotten first.
const MAX_ENTRIES: usize = 32;
const GOAL_PREVIEW_CHARS: usize = 60;
const ANSWER_PREVIEW_CHARS: usize = 80;
const MAX_ANSWER_BYTES: usize = 64 * 1024;

/// Starts one Run to completion on the calling thread. `started` receives the Run ID as soon as
/// the Run exists; `progress` is polled at every driver boundary.
pub(crate) type Launch = dyn Fn(
        String,
        InvocationGrants,
        &mut dyn FnMut(&str),
        &mut dyn FnMut(DriverProgress) -> DriverProgressControl,
    ) -> Result<LocalCommandResult, ReplFailure>
    + Send
    + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Running,
    Completed,
    Answered,
    Paused(&'static str),
    Cancelled,
    Rejected(&'static str),
    RecoveryRequired(&'static str),
    Failed(&'static str),
}

impl Status {
    fn label(&self) -> String {
        match self {
            Self::Running => "running".to_owned(),
            Self::Completed => "completed".to_owned(),
            Self::Answered => "answered".to_owned(),
            Self::Paused(reason) => format!("paused ({reason})"),
            Self::Cancelled => "cancelled".to_owned(),
            Self::Rejected(reason) => format!("rejected ({reason})"),
            Self::RecoveryRequired(reason) => format!("recovery required ({reason})"),
            Self::Failed(code) => format!("failed ({code})"),
        }
    }
}

struct Entry {
    label: String,
    goal: String,
    run_id: Option<String>,
    status: Status,
    answer: Option<String>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    elapsed: Option<Duration>,
}

#[derive(Default)]
struct State {
    launcher: Option<Arc<Launch>>,
    grants: InvocationGrants,
    next: u32,
    entries: VecDeque<Entry>,
    notices: VecDeque<String>,
    /// The terminal frontend prints notices as they arrive; otherwise the REPL prints them
    /// before reading the next line.
    frontend: bool,
}

/// Session-wide table of background Runs, shared by the REPL and the terminal frontend.
#[derive(Clone, Default)]
pub(crate) struct BackgroundRuns(Arc<Mutex<State>>);

impl BackgroundRuns {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn set_launcher(&self, launcher: Option<Arc<Launch>>) {
        self.state().launcher = launcher;
    }

    /// The approvals a background Run may use: what the session already granted.
    pub(crate) fn set_grants(&self, grants: InvocationGrants) {
        self.state().grants = grants;
    }

    pub(crate) fn grants(&self) -> InvocationGrants {
        self.state().grants
    }

    pub(crate) fn attach_frontend(&self) {
        self.state().frontend = true;
    }

    pub(crate) fn frontend(&self) -> bool {
        self.state().frontend
    }

    pub(crate) fn running(&self) -> usize {
        self.state()
            .entries
            .iter()
            .filter(|entry| entry.status == Status::Running)
            .count()
    }

    pub(crate) fn take_notices(&self) -> Vec<String> {
        self.state().notices.drain(..).collect()
    }

    /// Start `goal` as an independent Run. Returns the lines to show.
    pub(crate) fn spawn(&self, goal: &str, grants: InvocationGrants) -> Vec<String> {
        let goal = goal.trim();
        if goal.is_empty() {
            return vec!["error: background_goal_required (/bg GOAL)".to_owned()];
        }
        if !grants.model {
            return vec![
                "error: background_run_needs_model_permission (/permissions model allow)"
                    .to_owned(),
            ];
        }
        let mut state = self.state();
        let Some(launcher) = state.launcher.clone() else {
            return vec!["error: background_runs_unavailable".to_owned()];
        };
        if state
            .entries
            .iter()
            .filter(|entry| entry.status == Status::Running)
            .count()
            >= MAX_RUNNING
        {
            return vec![format!(
                "error: background_run_limit ({MAX_RUNNING} running; /runs, /cancel ID)"
            )];
        }
        state.next += 1;
        let label = format!("bg{}", state.next);
        let cancel = Arc::new(AtomicBool::new(false));
        state.entries.push_back(Entry {
            label: label.clone(),
            goal: preview(goal, GOAL_PREVIEW_CHARS),
            run_id: None,
            status: Status::Running,
            answer: None,
            cancel: Arc::clone(&cancel),
            started: Instant::now(),
            elapsed: None,
        });
        forget_old(&mut state.entries);
        drop(state);

        let table = self.clone();
        let goal = goal.to_owned();
        let thread_label = label.clone();
        let spawned = thread::Builder::new()
            .name(format!("xgen-{label}"))
            .spawn(move || {
                xgen_cli::agy::detach_from_session_cancellation();
                let mut started = |run_id: &str| table.record_run_id(&thread_label, run_id);
                let mut progress = |_event| {
                    if cancel.load(Ordering::SeqCst) {
                        DriverProgressControl::Cancel
                    } else {
                        DriverProgressControl::Continue
                    }
                };
                let result = launcher(goal, grants, &mut started, &mut progress);
                table.finish(&thread_label, result);
            });
        if spawned.is_err() {
            self.finish(
                &label,
                Err(ReplFailure::new("background_thread_unavailable")),
            );
            return Vec::new();
        }
        vec![format!(
            "[{label}] started in the background · /runs to list, /cancel {label} to stop"
        )]
    }

    fn record_run_id(&self, label: &str, run_id: &str) {
        if let Some(entry) = self.state().entries.iter_mut().find(|e| e.label == label) {
            entry.run_id = Some(run_id.to_owned());
        }
    }

    fn finish(&self, label: &str, result: Result<LocalCommandResult, ReplFailure>) {
        let mut state = self.state();
        let Some(entry) = state.entries.iter_mut().find(|e| e.label == label) else {
            return;
        };
        entry.elapsed = Some(entry.started.elapsed());
        let (status, run_id, answer) = classify(result);
        if run_id.is_some() {
            entry.run_id = run_id;
        }
        entry.status = status;
        entry.answer = answer;
        let notice = finish_notice(entry);
        state.notices.push_back(notice);
    }

    /// Handle `/runs`, `/show ID` and `/cancel ID`; `None` when `line` is not one of them.
    pub(crate) fn command(&self, line: &str) -> Option<Vec<String>> {
        let parts = line.split_ascii_whitespace().collect::<Vec<_>>();
        match parts.as_slice() {
            ["/runs"] => Some(self.list()),
            ["/show", id] => Some(self.show(id)),
            ["/cancel", id] => Some(self.cancel(id)),
            ["/show" | "/cancel"] => Some(vec!["error: background_id_required".to_owned()]),
            _ => None,
        }
    }

    fn list(&self) -> Vec<String> {
        let state = self.state();
        if state.entries.is_empty() {
            return vec!["background runs: none (/bg GOAL starts one)".to_owned()];
        }
        let mut lines = vec![format!(
            "background runs: {} running of {MAX_RUNNING}",
            state
                .entries
                .iter()
                .filter(|entry| entry.status == Status::Running)
                .count()
        )];
        for entry in &state.entries {
            let seconds = entry
                .elapsed
                .unwrap_or_else(|| entry.started.elapsed())
                .as_secs();
            lines.push(format!(
                "  {} {} {}s {} · {}",
                entry.label,
                entry.status.label(),
                seconds,
                entry.run_id.as_deref().unwrap_or("-"),
                entry.goal
            ));
        }
        lines
    }

    fn show(&self, id: &str) -> Vec<String> {
        let state = self.state();
        let Some(entry) = find(&state.entries, id) else {
            return vec!["error: background_run_unknown (/runs)".to_owned()];
        };
        let mut lines = vec![format!(
            "[{}] {} {} · {}",
            entry.label,
            entry.status.label(),
            entry.run_id.as_deref().unwrap_or("-"),
            entry.goal
        )];
        match (&entry.status, &entry.answer) {
            (_, Some(answer)) => lines.extend(answer.lines().map(terminal_line)),
            (Status::Running, None) => lines.push("still running".to_owned()),
            (_, None) => {
                if let Some(run_id) = &entry.run_id {
                    lines.push(format!(
                        "no answer was saved; continue with /resume {run_id}"
                    ));
                } else {
                    lines.push("no answer was saved".to_owned());
                }
            }
        }
        lines
    }

    fn cancel(&self, id: &str) -> Vec<String> {
        let state = self.state();
        match find(&state.entries, id) {
            Some(entry) if entry.status == Status::Running => {
                entry.cancel.store(true, Ordering::SeqCst);
                vec![format!(
                    "[{}] cancelling at the next safe boundary",
                    entry.label
                )]
            }
            Some(entry) => vec![format!(
                "[{}] is not running: {}",
                entry.label,
                entry.status.label()
            )],
            None => vec!["error: background_run_unknown (/runs)".to_owned()],
        }
    }

    /// Ask every running background Run to stop at its next boundary.
    pub(crate) fn cancel_all(&self) {
        for entry in &self.state().entries {
            if entry.status == Status::Running {
                entry.cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Wait until no background Run is running, or `limit` passes.
    pub(crate) fn wait_idle(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.running() > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
        true
    }
}

/// Match a label (`bg2`) or the durable Run ID of an entry.
fn find<'a>(entries: &'a VecDeque<Entry>, id: &str) -> Option<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.label == id || entry.run_id.as_deref() == Some(id))
}

/// Keep at most `MAX_ENTRIES`, dropping the oldest finished ones; running entries stay.
fn forget_old(entries: &mut VecDeque<Entry>) {
    while entries.len() > MAX_ENTRIES {
        let Some(index) = entries
            .iter()
            .position(|entry| entry.status != Status::Running)
        else {
            return;
        };
        entries.remove(index);
    }
}

fn classify(
    result: Result<LocalCommandResult, ReplFailure>,
) -> (Status, Option<String>, Option<String>) {
    match result {
        Ok(LocalCommandResult::Completed { run_id, summary }) => {
            (Status::Completed, Some(run_id), Some(bounded(summary)))
        }
        Ok(LocalCommandResult::Responded { run_id, summary }) => {
            (Status::Answered, Some(run_id), Some(bounded(summary)))
        }
        Ok(LocalCommandResult::Paused { run_id, reason }) => {
            let status = if reason == xgen_cli::PauseReason::UserCancelled {
                Status::Cancelled
            } else {
                Status::Paused(reason.code())
            };
            (status, run_id, None)
        }
        Ok(LocalCommandResult::Rejected {
            run_id,
            reason,
            provider,
        }) => {
            let answer = provider.map(|rejection| format!("Provider said: {rejection}"));
            (
                Status::Rejected(rejection_code(reason)),
                Some(run_id),
                answer,
            )
        }
        Ok(LocalCommandResult::RecoveryRequired { run_id, reason }) => {
            (Status::RecoveryRequired(reason.code()), Some(run_id), None)
        }
        Err(failure) => (Status::Failed(failure.code()), None, None),
    }
}

const fn rejection_code(reason: RejectionReason) -> &'static str {
    reason.code()
}

fn finish_notice(entry: &Entry) -> String {
    let run = entry.run_id.as_deref().unwrap_or("-");
    let label = &entry.label;
    match &entry.status {
        Status::Completed | Status::Answered => {
            let first = entry
                .answer
                .as_deref()
                .and_then(|answer| answer.lines().find(|line| !line.trim().is_empty()))
                .map(|line| preview(line, ANSWER_PREVIEW_CHARS))
                .unwrap_or_default();
            format!(
                "[{label}] {} {run}: {first} · /show {label}",
                entry.status.label()
            )
        }
        Status::Paused(_) | Status::RecoveryRequired(_) => {
            format!("[{label}] {} {run} · /resume {run}", entry.status.label())
        }
        Status::Rejected(_) => format!("[{label}] {} {run} · /show {label}", entry.status.label()),
        Status::Cancelled | Status::Failed(_) | Status::Running => {
            format!("[{label}] {} {run}", entry.status.label())
        }
    }
}

fn bounded(text: String) -> String {
    if text.len() <= MAX_ANSWER_BYTES {
        return text;
    }
    let mut end = MAX_ANSWER_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... (truncated; /resume shows the full Run)",
        &text[..end]
    )
}

/// A model-written line made safe to print: control characters become spaces.
fn terminal_line(line: &str) -> String {
    line.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// One terminal-safe line: control characters become spaces and long text is cut.
fn preview(text: &str, limit: usize) -> String {
    let flat = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        format!("{}...", flat.chars().take(limit).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn grants() -> InvocationGrants {
        InvocationGrants {
            model: true,
            ..InvocationGrants::default()
        }
    }

    /// Each launched goal blocks until the test releases it with the result to return.
    fn scripted() -> (
        BackgroundRuns,
        mpsc::Receiver<(String, mpsc::Sender<LocalCommandResult>)>,
    ) {
        let (calls, received) = mpsc::channel();
        let calls = Mutex::new(calls);
        let runs = BackgroundRuns::default();
        let launch: Arc<Launch> = Arc::new(move |goal, _grants, started, progress| {
            let run_id = format!("run-{}", goal.replace(' ', "-"));
            started(&run_id);
            let (reply, result) = mpsc::channel();
            calls.lock().unwrap().send((goal, reply)).unwrap();
            loop {
                if let Ok(result) = result.try_recv() {
                    return Ok(result);
                }
                if progress(DriverProgress::ModelCallStarting) == DriverProgressControl::Cancel {
                    return Ok(LocalCommandResult::Paused {
                        run_id: Some(run_id),
                        reason: xgen_cli::PauseReason::UserCancelled,
                    });
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        runs.set_launcher(Some(launch));
        (runs, received)
    }

    fn wait_for_notice(runs: &BackgroundRuns) -> Vec<String> {
        for _ in 0..2000 {
            let notices = runs.take_notices();
            if !notices.is_empty() {
                return notices;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("no notice arrived");
    }

    #[test]
    fn runs_proceed_independently_and_report_when_they_finish() {
        let (runs, calls) = scripted();
        assert!(runs.spawn("first goal", grants())[0].starts_with("[bg1] started"));
        assert!(runs.spawn("second goal", grants())[0].starts_with("[bg2] started"));
        let (first, first_reply) = calls.recv().unwrap();
        let (second, second_reply) = calls.recv().unwrap();
        let mut goals = [first.clone(), second.clone()];
        goals.sort();
        assert_eq!(goals, ["first goal", "second goal"]);
        assert_eq!(runs.running(), 2);

        // The second finishes first; the first is unaffected.
        let (reply, other) = if second == "second goal" {
            (second_reply, first_reply)
        } else {
            (first_reply, second_reply)
        };
        reply
            .send(LocalCommandResult::Responded {
                run_id: "run-second-goal".to_owned(),
                summary: "\nsecond answer line\nmore".to_owned(),
            })
            .unwrap();
        let notices = wait_for_notice(&runs);
        assert_eq!(
            notices,
            ["[bg2] answered run-second-goal: second answer line · /show bg2"]
        );
        assert_eq!(runs.running(), 1);
        assert_eq!(
            runs.command("/show bg2").unwrap(),
            [
                "[bg2] answered run-second-goal · second goal",
                "",
                "second answer line",
                "more"
            ]
        );
        assert_eq!(
            runs.command("/show run-second-goal").unwrap()[0],
            "[bg2] answered run-second-goal · second goal"
        );

        other
            .send(LocalCommandResult::Paused {
                run_id: Some("run-first-goal".to_owned()),
                reason: xgen_cli::PauseReason::WriteApprovalRequired,
            })
            .unwrap();
        let notices = wait_for_notice(&runs);
        assert_eq!(
            notices,
            ["[bg1] paused (write_approval_required) run-first-goal · /resume run-first-goal"]
        );
        let list = runs.command("/runs").unwrap();
        assert_eq!(list[0], "background runs: 0 running of 3");
        assert!(list[1].starts_with("  bg1 paused (write_approval_required) "));
        assert!(list[2].starts_with("  bg2 answered "));
    }

    #[test]
    fn cancel_stops_only_the_named_run_and_the_limit_holds() {
        let (runs, calls) = scripted();
        for goal in ["a", "b", "c"] {
            runs.spawn(goal, grants());
        }
        let replies = (0..3).map(|_| calls.recv().unwrap()).collect::<Vec<_>>();
        assert_eq!(
            runs.spawn("d", grants()),
            ["error: background_run_limit (3 running; /runs, /cancel ID)"]
        );
        assert_eq!(
            runs.command("/cancel bg2").unwrap(),
            ["[bg2] cancelling at the next safe boundary"]
        );
        assert_eq!(wait_for_notice(&runs), ["[bg2] cancelled run-b"]);
        assert_eq!(runs.running(), 2);
        assert_eq!(
            runs.command("/cancel bg2").unwrap(),
            ["[bg2] is not running: cancelled"]
        );
        assert!(runs.spawn("d", grants())[0].starts_with("[bg4] started"));
        runs.cancel_all();
        assert!(runs.wait_idle(Duration::from_secs(5)));
        drop(replies);
    }

    #[test]
    fn refusals_need_a_goal_model_permission_and_a_launcher() {
        let runs = BackgroundRuns::default();
        assert_eq!(
            runs.spawn("goal", grants()),
            ["error: background_runs_unavailable"]
        );
        assert_eq!(
            runs.spawn("goal", InvocationGrants::default()),
            ["error: background_run_needs_model_permission (/permissions model allow)"]
        );
        assert_eq!(
            runs.spawn("  ", grants()),
            ["error: background_goal_required (/bg GOAL)"]
        );
        assert_eq!(
            runs.command("/runs").unwrap(),
            ["background runs: none (/bg GOAL starts one)"]
        );
        assert_eq!(
            runs.command("/show bg9").unwrap(),
            ["error: background_run_unknown (/runs)"]
        );
        assert!(runs.command("/status").is_none());
    }

    #[test]
    fn previews_are_single_terminal_lines() {
        assert_eq!(preview("a\u{1b}[31mb\nc\td", 80), "a [31mb c d");
        assert_eq!(
            preview(&"x".repeat(90), 10),
            format!("{}...", "x".repeat(10))
        );
        let long = "가".repeat(MAX_ANSWER_BYTES);
        assert!(bounded(long).ends_with("(truncated; /resume shows the full Run)"));
    }
}
