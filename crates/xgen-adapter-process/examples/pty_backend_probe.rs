//! Offline PTY adoption probe; not registered as a production Capability.

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    use nix::unistd::Pid;
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
    use serde_json::{Value, json};

    const DEADLINE: Duration = Duration::from_secs(5);
    const OUTPUT_LIMIT: usize = 1024;

    struct Session {
        child: Box<dyn Child + Send + Sync>,
        master: Box<dyn MasterPty + Send>,
        writer: Option<Box<dyn Write + Send>>,
        output: Receiver<Vec<u8>>,
        group: Option<Pid>,
        reaped: bool,
    }

    impl Session {
        fn spawn(mode: &str, directory: &Path) -> Self {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open PTY");
            let mut command = CommandBuilder::new(std::env::current_exe().expect("probe binary"));
            command.args(["fixture", mode, "literal;$(touch marker)"]);
            command.cwd(directory);
            command.env_clear();
            command.env("PATH", "/usr/bin:/bin");
            let child = pair.slave.spawn_command(command).expect("spawn fixture");
            let group =
                Pid::from_raw(i32::try_from(child.process_id().expect("child PID")).unwrap());
            drop(pair.slave);
            let writer = pair.master.take_writer().expect("writer");
            let mut reader = pair.master.try_clone_reader().expect("reader");
            let (sender, output) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let mut retained = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(size) => {
                            let keep = size.min(OUTPUT_LIMIT.saturating_sub(retained.len()));
                            retained.extend_from_slice(&buffer[..keep]);
                        }
                    }
                }
                let _ = sender.send(retained);
            });
            Self {
                child,
                master: pair.master,
                writer: Some(writer),
                output,
                group: Some(group),
                reaped: false,
            }
        }

        fn wait(&mut self) -> WaitStatus {
            let started = Instant::now();
            loop {
                let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT;
                let group = self.group.expect("owned leader group");
                let status = match waitid(Id::Pid(group), flags) {
                    Ok(status) => status,
                    Err(Errno::ECHILD) => {
                        // portable-pty Child::kill may reap its child internally. Only the
                        // adversarial tree fixtures use that path; their descendants remain
                        // alive until explicit group cleanup, retaining the owned group ID.
                        self.reaped = true;
                        let status = self
                            .child
                            .try_wait()
                            .unwrap()
                            .expect("library-reaped child");
                        return WaitStatus::Exited(
                            group,
                            i32::try_from(status.exit_code()).unwrap(),
                        );
                    }
                    Err(error) => panic!("observe unreaped child: {error}"),
                };
                if !matches!(status, WaitStatus::StillAlive) {
                    return status;
                }
                assert!(started.elapsed() < DEADLINE, "child deadline exceeded");
                thread::sleep(Duration::from_millis(5));
            }
        }

        fn finish(&mut self) -> Vec<u8> {
            self.writer.take();
            self.kill_group();
            self.child.wait().expect("reap after group cleanup");
            self.reaped = true;
            self.output.recv_timeout(DEADLINE).expect("bounded drain")
        }

        fn kill_group(&mut self) {
            if let Some(group) = self.group.take() {
                let _ = killpg(group, Signal::SIGKILL);
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // This PTY creates a new session/group. Only that owned group is signalled.
            self.kill_group();
            self.writer.take();
            if !self.reaped {
                let _ = self.child.wait();
            }
        }
    }

    fn wait_file(path: &Path) {
        let started = Instant::now();
        while !path.is_file() {
            assert!(
                started.elapsed() < DEADLINE,
                "fixture readiness deadline exceeded"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn fixture(mode: &str) {
        match mode {
            "input" => {
                println!("READY");
                std::io::stdout().flush().unwrap();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                println!("REPLY:{}", input.trim());
                println!("ARG:{}", std::env::args().nth(3).unwrap());
            }
            "output" => {
                for _ in 0..8192 {
                    println!("한글-output");
                }
            }
            "failure" => std::process::exit(7),
            "tree" | "nested-tree" => {
                // Shell syntax exists only in this adversarial test fixture, never model input.
                let body = if mode == "tree" {
                    "trap '' HUP; : > ready; sleep 1; : > escaped; sleep 10"
                } else {
                    "trap '' HUP; /bin/sh -c 'sleep 1; : > escaped; sleep 10' & : > ready; wait"
                };
                let mut descendant = std::process::Command::new("/bin/sh")
                    .args(["-c", body])
                    .spawn()
                    .unwrap();
                let _ = descendant.wait();
            }
            _ => panic!("unknown fixture"),
        }
    }

    pub fn run() {
        let args: Vec<_> = std::env::args().collect();
        if args.get(1).is_some_and(|value| value == "fixture") {
            fixture(&args[2]);
            return;
        }
        let mut records: Vec<Value> = Vec::new();
        for input in ["hello", "한글 입력"] {
            let directory = tempfile::tempdir().unwrap();
            let mut session = Session::spawn("input", directory.path());
            session
                .master
                .resize(PtySize {
                    rows: 30,
                    cols: 100,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            let size = session.master.get_size().unwrap();
            session
                .writer
                .as_mut()
                .unwrap()
                .write_all(format!("{input}\n").as_bytes())
                .unwrap();
            session.writer.as_mut().unwrap().flush().unwrap();
            let status = session.wait();
            let output = String::from_utf8(session.finish()).unwrap();
            let pass = matches!(status, WaitStatus::Exited(_, 0))
                && output.contains(&format!("REPLY:{input}"))
                && output.contains("ARG:literal;$(touch marker)")
                && !directory.path().join("marker").exists()
                && size.rows == 30
                && size.cols == 100;
            records
                .push(json!({"case": "input_resize_literal_argv", "input": input, "passed": pass}));
            assert!(pass);
        }
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::spawn("output", directory.path());
        let pass = matches!(session.wait(), WaitStatus::Exited(_, 0));
        let output = session.finish();
        records.push(json!({"case": "drain_beyond_retention_limit", "passed": pass && output.len() == OUTPUT_LIMIT, "retained_bytes": output.len()}));
        assert!(pass && output.len() == OUTPUT_LIMIT);
        let mut session = Session::spawn("failure", directory.path());
        let status = session.wait();
        session.finish();
        let pass = matches!(status, WaitStatus::Exited(_, 7));
        records.push(json!({"case": "nonzero_exit", "passed": pass}));
        assert!(pass);
        for mode in ["tree", "nested-tree"] {
            for group_cancel in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let mut session = Session::spawn(mode, directory.path());
                wait_file(&directory.path().join("ready"));
                if group_cancel {
                    killpg(session.group.unwrap(), Signal::SIGKILL).unwrap();
                } else {
                    session.child.kill().unwrap();
                }
                session.wait();
                // Allow the descendant's marker deadline to pass before cleanup.
                thread::sleep(Duration::from_millis(1250));
                let escaped = directory.path().join("escaped").exists();
                session.finish();
                records.push(json!({"case": mode, "cancel": if group_cancel { "owned_process_group" } else { "portable_pty_child_kill" }, "descendant_escaped": escaped, "passed": !escaped}));
                if group_cancel {
                    assert!(!escaped);
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&json!({"backend": "portable-pty", "version": "0.9.0", "platform": std::env::consts::OS, "production_registered": false, "cases": records})).unwrap());
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    linux::run();
    #[cfg(not(target_os = "linux"))]
    println!(
        "{{\"status\":\"not_validated\",\"reason\":\"owned process-group adoption probe is Linux only\"}}"
    );
}
