use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use super::{TerminalOperation, snapshot};
use crate::{ProcessWorkspace, execution::PreparedProcess};
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
use nix::unistd::Pid;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::Value;

const POLL: Duration = Duration::from_millis(10);
const MAX_SESSIONS: usize = 8;

#[derive(Default)]
pub(crate) struct Sessions {
    entries: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
}
impl Sessions {
    pub(crate) fn execute(
        &self,
        operation: TerminalOperation,
        input: &Value,
        handle: &str,
        workspace: &ProcessWorkspace,
    ) -> Value {
        if operation == TerminalOperation::Start {
            return self.start(input, handle, workspace);
        }
        let id = input["sessionId"].as_str().expect("validated handle");
        let session = self
            .entries
            .lock()
            .expect("session registry lock")
            .get(id)
            .cloned();
        let Some(session) = session else {
            return snapshot(id, "session_lost");
        };
        match operation {
            TerminalOperation::Start => unreachable!(),
            TerminalOperation::Read => read(&session, id, input),
            TerminalOperation::Write => {
                let mut session = session.lock().expect("session lock");
                session.tick();
                let (accepted, state) =
                    session.write(input["input"].as_str().expect("validated input").as_bytes());
                let mut result = session.snapshot(id);
                result["acceptedBytes"] = accepted.into();
                if matches!(state, "input_partial" | "input_failed") {
                    result["state"] = state.into();
                }
                result
            }
            TerminalOperation::Terminate => {
                let mut session = session.lock().expect("session lock");
                session.tick();
                if session.child.is_some() {
                    session.stop("terminated");
                }
                session.snapshot(id)
            }
        }
    }
    fn start(&self, input: &Value, handle: &str, workspace: &ProcessWorkspace) -> Value {
        let mut entries = self.entries.lock().expect("session registry lock");
        // Duplicate preparation in the same host never spawns a second command.
        if let Some(session) = entries.get(handle) {
            return session.lock().expect("session lock").snapshot(handle);
        }
        if entries.len() >= MAX_SESSIONS {
            return snapshot(handle, "session_limit");
        }
        let Ok(prepared) = crate::execution::parse_arguments(input, workspace) else {
            return snapshot(handle, "launch_failed");
        };
        let session = match Session::spawn(&prepared) {
            Ok(session) => Arc::new(Mutex::new(session)),
            Err(()) => return snapshot(handle, "launch_failed"),
        };
        let weak = Arc::downgrade(&session);
        thread::spawn(move || {
            loop {
                let Some(session) = weak.upgrade() else {
                    break;
                };
                let done = {
                    let mut session = session.lock().expect("session lock");
                    session.tick();
                    session.child.is_none()
                };
                drop(session);
                if done {
                    break;
                }
                thread::sleep(POLL);
            }
        });
        let result = session.lock().expect("session lock").snapshot(handle);
        entries.insert(handle.to_owned(), session);
        result
    }
}
impl Drop for Sessions {
    fn drop(&mut self) {
        for session in self
            .entries
            .get_mut()
            .expect("session registry lock")
            .values()
        {
            session.lock().expect("session lock").stop("terminated");
        }
    }
}

struct Buffer {
    bytes: VecDeque<u8>,
    base: u64,
    end: u64,
    limit: usize,
    closed: bool,
}
impl Buffer {
    fn append(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if self.bytes.len() == self.limit {
                self.bytes.pop_front();
                self.base = self.base.saturating_add(1);
            }
            self.bytes.push_back(*byte);
            self.end = self.end.saturating_add(1);
        }
    }
}
struct Session {
    child: Option<Box<dyn Child + Send + Sync>>,
    group: Option<Pid>,
    _master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    buffer: Arc<Mutex<Buffer>>,
    stop_reader: Arc<AtomicBool>,
    deadline: Instant,
    state: &'static str,
    exit_code: Option<u32>,
}
impl Session {
    fn spawn(prepared: &PreparedProcess) -> Result<Self, ()> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| ())?;
        let fd = pair.master.as_raw_fd().ok_or(())?;
        let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(|_| ())?;
        fcntl(
            fd,
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(|_| ())?;
        let writer = pair.master.take_writer().map_err(|_| ())?;
        let reader = pair.master.try_clone_reader().map_err(|_| ())?;
        let mut command = CommandBuilder::new(&prepared.executable);
        command.args(&prepared.args);
        command.cwd(&prepared.cwd);
        command.env_clear();
        for (key, value) in &prepared.environment {
            command.env(key, value);
        }
        let child = pair.slave.spawn_command(command).map_err(|_| ())?;
        // Linux portable-pty spawns a fresh session whose leader is this unreaped child.
        let pid = child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok())
            .expect("native Linux child PID");
        drop(pair.slave);
        let buffer = Arc::new(Mutex::new(Buffer {
            bytes: VecDeque::new(),
            base: 0,
            end: 0,
            limit: prepared.max_output_bytes,
            closed: false,
        }));
        let stop_reader = Arc::new(AtomicBool::new(false));
        capture(reader, Arc::clone(&buffer), Arc::clone(&stop_reader));
        Ok(Self {
            child: Some(child),
            group: Some(Pid::from_raw(pid)),
            _master: pair.master,
            writer: Some(writer),
            buffer,
            stop_reader,
            deadline: Instant::now() + prepared.timeout,
            state: "running",
            exit_code: None,
        })
    }
    fn tick(&mut self) {
        if self.child.is_none() {
            return;
        }
        if let Some(group) = self.group {
            let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT;
            match waitid(Id::Pid(group), flags) {
                Ok(WaitStatus::StillAlive) => {
                    if Instant::now() >= self.deadline {
                        self.stop("timed_out");
                    }
                }
                Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => self.stop("exited"),
                _ => {
                    self.group.take();
                    self.state = "cleanup_failed";
                    return;
                }
            }
        }
        if self.group.is_none() {
            match self.child.as_mut().expect("unreaped child").try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = Some(status.exit_code());
                    self.child.take();
                    self.writer.take();
                }
                Ok(None) => {}
                Err(_) => self.state = "cleanup_failed",
            }
        }
    }
    fn stop(&mut self, state: &'static str) {
        if let Some(group) = self.group {
            // Never signal a numeric group after the leader has been reaped elsewhere.
            let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT;
            if waitid(Id::Pid(group), flags).is_err() {
                self.group.take();
                self.state = "cleanup_failed";
                return;
            }
            match killpg(group, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {
                    self.group.take();
                    self.state = state;
                }
                Err(_) => {
                    self.state = "cleanup_failed";
                    return;
                }
            }
        }
        self.writer.take();
    }
    fn snapshot(&self, handle: &str) -> Value {
        let buffer = self.buffer.lock().expect("output buffer lock");
        let state =
            if self.state != "cleanup_failed" && self.child.is_some() && self.group.is_none() {
                "stopping"
            } else {
                self.state
            };
        let state = if self.state != "cleanup_failed" && self.child.is_none() && !buffer.closed {
            "draining"
        } else {
            state
        };
        let mut result = snapshot(handle, state);
        result["exitCode"] = self.exit_code.into();
        result["baseOffset"] = buffer.base.into();
        result["nextOffset"] = buffer.base.into();
        result["truncated"] = (buffer.base != 0).into();
        result
    }
    fn write(&mut self, input: &[u8]) -> (usize, &'static str) {
        if self.child.is_some() && self.group.is_none() {
            return (0, "stopping");
        }
        if self.state != "running" {
            return (0, self.state);
        }
        let Some(writer) = self.writer.as_mut() else {
            return (0, "input_failed");
        };
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut accepted = 0;
        while accepted < input.len() {
            if Instant::now() >= deadline {
                return (accepted, "input_partial");
            }
            match writer.write(&input[accepted..]) {
                Ok(0) => return (accepted, "input_failed"),
                Ok(size) => accepted += size,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return (accepted, "input_partial");
                    }
                    thread::sleep(POLL);
                }
                Err(_) => return (accepted, "input_failed"),
            }
        }
        (accepted, "running")
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop("terminated");
        self.stop_reader.store(true, Ordering::Release);
        if let Some(mut child) = self.child.take() {
            // Reaping cannot block an adapter or host exit; group ownership was released first.
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
fn capture(mut reader: Box<dyn Read + Send>, buffer: Arc<Mutex<Buffer>>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut bytes = [0; 4096];
        while !stop.load(Ordering::Acquire) {
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(size) => buffer
                    .lock()
                    .expect("output buffer lock")
                    .append(&bytes[..size]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        buffer.lock().expect("output buffer lock").closed = true;
    });
}
fn read(session: &Mutex<Session>, handle: &str, input: &Value) -> Value {
    let offset = input["offset"].as_u64().expect("validated offset");
    let maximum = usize::try_from(input["maxBytes"].as_u64().expect("validated limit"))
        .expect("bounded limit");
    let deadline =
        Instant::now() + Duration::from_millis(input["waitMs"].as_u64().expect("validated wait"));
    loop {
        let mut session = session.lock().expect("session lock");
        session.tick();
        let mut result = session.snapshot(handle);
        let buffer = session.buffer.lock().expect("output buffer lock");
        if offset > buffer.end {
            result["state"] = "offset_invalid".into();
            return result;
        }
        if buffer.end > offset || buffer.closed || Instant::now() >= deadline {
            let start = offset.max(buffer.base);
            let skip = usize::try_from(start - buffer.base).expect("bounded ring offset");
            let bytes: Vec<u8> = buffer
                .bytes
                .iter()
                .skip(skip)
                .take(maximum)
                .copied()
                .collect();
            result["output"] = String::from_utf8_lossy(&bytes).into_owned().into();
            result["nextOffset"] =
                (start + u64::try_from(bytes.len()).expect("bounded output")).into();
            result["truncated"] = (offset < buffer.base).into();
            return result;
        }
        drop(buffer);
        drop(session);
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutableCatalog, ProcessEnvironment, ProcessWorkspaceId};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    #[allow(clippy::zombie_processes)] // The adversarial fixture intentionally leaves an owned descendant for group cleanup.
    fn terminal_child_fixture() {
        let Ok(mode) = std::env::var("XGEN_PTY_TEST_MODE") else {
            return;
        };
        match mode.as_str() {
            "input" => {
                println!("READY");
                std::io::stdout().flush().unwrap();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                println!("REPLY:{}", input.trim());
                println!("LITERAL:{}", std::env::args().next_back().unwrap());
            }
            "output" => {
                for _ in 0..10000 {
                    println!("한글-output");
                }
            }
            "failure" => std::process::exit(7),
            "tree" | "orphan" => {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "terminal::sessions::tests::terminal_child_fixture",
                        "--exact",
                        "--nocapture",
                    ])
                    .env("XGEN_PTY_TEST_MODE", "sleep")
                    .spawn()
                    .unwrap();
                std::fs::write("descendant.pid", child.id().to_string()).unwrap();
                if mode == "tree" {
                    thread::sleep(Duration::from_secs(30));
                }
            }
            "sleep" => thread::sleep(Duration::from_secs(30)),
            _ => panic!("unknown fixture"),
        }
    }
    fn workspace(directory: &std::path::Path) -> ProcessWorkspace {
        ProcessWorkspace::open_ambient(
            directory,
            ProcessWorkspaceId::new("fixture").unwrap(),
            ExecutableCatalog::from_paths([("helper", std::env::current_exe().unwrap())]).unwrap(),
            ProcessEnvironment::new(BTreeMap::new()).unwrap(),
        )
        .unwrap()
    }
    fn input(mode: &str, lifetime: u64) -> Value {
        json!({"executable":"process:fixture/executables/helper", "args":["terminal::sessions::tests::terminal_child_fixture", "--exact", "--nocapture", "literal;$(touch marker)"], "cwd":".", "env":{"XGEN_PTY_TEST_MODE":mode}, "timeoutMs":lifetime, "maxOutputBytes":1024})
    }
    fn handle(index: u8) -> String {
        format!("pty-{index:064x}")
    }
    fn observe(workspace: &ProcessWorkspace, id: &str, offset: u64) -> Value {
        let output = workspace.terminals.execute(
            TerminalOperation::Read,
            &json!({"sessionId":id,"offset":offset,"maxBytes":4096,"waitMs":100}),
            "unused",
            workspace,
        );
        assert!(
            super::super::inspect_output(&output, None).is_ok(),
            "{output}"
        );
        output
    }
    fn settled(workspace: &ProcessWorkspace, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let output = observe(workspace, id, 0);
            if !matches!(
                output["state"].as_str(),
                Some("running" | "stopping" | "draining")
            ) && workspace
                .terminals
                .entries
                .lock()
                .unwrap()
                .get(id)
                .unwrap()
                .lock()
                .unwrap()
                .buffer
                .lock()
                .unwrap()
                .closed
            {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "session failed to settle: {output}"
            );
            thread::sleep(POLL);
        }
    }
    #[test]
    fn literal_input_unicode_output_and_non_consuming_offsets() {
        for text in ["English response", "한국어 응답"] {
            let directory = tempfile::tempdir().unwrap();
            let workspace = workspace(directory.path());
            let id = handle(1);
            let started = workspace.terminals.execute(
                TerminalOperation::Start,
                &input("input", 5000),
                &id,
                &workspace,
            );
            assert_eq!(started["state"], "running");
            let deadline = Instant::now() + Duration::from_secs(3);
            while !observe(&workspace, &id, 0)["output"]
                .as_str()
                .unwrap()
                .contains("READY")
            {
                assert!(Instant::now() < deadline);
            }
            let payload = format!("{text}\n");
            let written = workspace.terminals.execute(
                TerminalOperation::Write,
                &json!({"sessionId":id,"input":payload}),
                "unused",
                &workspace,
            );
            assert_eq!(written["acceptedBytes"], payload.len());
            let final_output = settled(&workspace, &id);
            assert!(
                final_output["output"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("REPLY:{text}"))
            );
            assert!(
                final_output["output"]
                    .as_str()
                    .unwrap()
                    .contains("literal;$(touch marker)")
            );
            assert_eq!(final_output, observe(&workspace, &id, 0));
            assert!(!directory.path().join("marker").exists());
        }
    }
    #[test]
    fn output_ring_is_bounded_and_reports_lost_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(directory.path());
        let id = handle(2);
        workspace.terminals.execute(
            TerminalOperation::Start,
            &input("output", 5000),
            &id,
            &workspace,
        );
        let output = settled(&workspace, &id);
        assert_eq!(output["truncated"], true);
        assert!(output["baseOffset"].as_u64().unwrap() > 0);
        assert!(output["output"].as_str().unwrap().len() <= 3072);
        let future = observe(&workspace, &id, u64::MAX);
        assert_eq!(future["state"], "offset_invalid");
    }
    #[test]
    fn nonzero_timeout_and_missing_session_remain_observations() {
        for (mode, lifetime, expected) in [("failure", 5000, "exited"), ("sleep", 100, "timed_out")]
        {
            let directory = tempfile::tempdir().unwrap();
            let workspace = workspace(directory.path());
            let id = handle(3);
            workspace.terminals.execute(
                TerminalOperation::Start,
                &input(mode, lifetime),
                &id,
                &workspace,
            );
            let output = settled(&workspace, &id);
            assert_eq!(output["state"], expected);
            if mode == "failure" {
                assert_eq!(output["exitCode"], 7);
            }
            let reopened = workspace.with_fresh_terminal_sessions();
            assert_eq!(observe(&reopened, &id, 0)["state"], "session_lost");
        }
    }
    fn alive(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(") ")
                    .map(|(_, status)| !status.starts_with('Z'))
            })
            .unwrap_or(false)
    }
    #[test]
    fn termination_normal_exit_and_host_drop_clean_owned_descendants() {
        for mode in ["tree", "orphan", "drop"] {
            let directory = tempfile::tempdir().unwrap();
            let workspace = workspace(directory.path());
            let id = handle(4);
            workspace.terminals.execute(
                TerminalOperation::Start,
                &input(if mode == "drop" { "tree" } else { mode }, 5000),
                &id,
                &workspace,
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            let path = directory.path().join("descendant.pid");
            while !path.is_file() {
                assert!(Instant::now() < deadline);
                thread::sleep(POLL);
            }
            let pid: i32 = std::fs::read_to_string(path).unwrap().parse().unwrap();
            if mode == "tree" {
                workspace.terminals.execute(
                    TerminalOperation::Terminate,
                    &json!({"sessionId":id}),
                    "unused",
                    &workspace,
                );
                assert_eq!(settled(&workspace, &id)["state"], "terminated");
            } else if mode == "orphan" {
                assert_eq!(settled(&workspace, &id)["state"], "exited");
            }
            drop(workspace);
            while alive(pid) {
                assert!(
                    Instant::now() < deadline,
                    "descendant survived owned group cleanup"
                );
                thread::sleep(POLL);
            }
        }
    }
    #[test]
    fn terminal_control_bytes_and_write_deadlines_are_supported() {
        for (mode, input) in [("input", "\u{4}"), ("sleep", "\u{3}")] {
            let directory = tempfile::tempdir().unwrap();
            let workspace = workspace(directory.path());
            let id = handle(5);
            workspace.terminals.execute(
                TerminalOperation::Start,
                &self::input(mode, 5000),
                &id,
                &workspace,
            );
            // Give the fixture time to install its foreground terminal; no input is inferred.
            thread::sleep(Duration::from_millis(50));
            let output = workspace.terminals.execute(
                TerminalOperation::Write,
                &json!({"sessionId":id,"input":input}),
                "unused",
                &workspace,
            );
            assert_eq!(output["acceptedBytes"], 1);
            assert_eq!(settled(&workspace, &id)["state"], "exited");
        }
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(directory.path());
        let id = handle(6);
        workspace.terminals.execute(
            TerminalOperation::Start,
            &input("sleep", 5000),
            &id,
            &workspace,
        );
        for _ in 0..20 {
            let start = Instant::now();
            let output = workspace.terminals.execute(
                TerminalOperation::Write,
                &json!({"sessionId":id,"input":"x".repeat(1024)}),
                "unused",
                &workspace,
            );
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(super::super::inspect_output(&output, None).is_ok());
        }
    }

    #[test]
    fn repeated_start_is_not_respawned_and_session_count_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(directory.path());
        for n in 0..8 {
            let id = handle(n);
            workspace.terminals.execute(
                TerminalOperation::Start,
                &input("failure", 5000),
                &id,
                &workspace,
            );
            settled(&workspace, &id);
        }
        assert_eq!(
            workspace.terminals.execute(
                TerminalOperation::Start,
                &input("sleep", 5000),
                &handle(9),
                &workspace
            )["state"],
            "session_limit"
        );
        assert_eq!(
            workspace.terminals.execute(
                TerminalOperation::Start,
                &input("sleep", 5000),
                &handle(0),
                &workspace
            )["state"],
            "exited"
        );
    }
}
