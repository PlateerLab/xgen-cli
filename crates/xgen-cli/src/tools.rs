//! Internal graph retrieval transport; it never grants invocation authority.

use std::env;
use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Subcommand;
#[cfg(windows)]
use process_wrap::std::JobObject;
#[cfg(unix)]
use process_wrap::std::ProcessGroup;
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde_json::{Value, json};

const WORKER: &str = include_str!("tool_graph_worker.py");
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// Import JSON `OpenAPI` documents or public HTTPS Swagger URLs as an immutable collection.
    Import {
        #[arg(long)]
        name: String,
        #[arg(long, required = true, num_args = 1..)]
        source: Vec<String>,
    },
    /// Connect an imported collection to a host-approved GET-only API base URL.
    Connect {
        #[arg(long)]
        name: String,
        #[arg(long)]
        base_url: String,
        /// Confirm that this system's GET operations are non-mutating read operations.
        #[arg(long, required = true)]
        allow_get: bool,
        /// Read the bearer token from a local environment variable at execution time.
        #[arg(long)]
        bearer_env: Option<String>,
        /// Read a bearer token locally with a hidden prompt and save it in the OS secret store.
        #[arg(long)]
        bearer: bool,
        /// Read the bearer token from stdin and save it in the OS secret store.
        #[arg(long)]
        token_stdin: bool,
    },
    /// List saved collections.
    List,
    /// Retrieve candidates and possible prerequisite producers; never executes them.
    Search {
        #[arg(long)]
        name: String,
        query: String,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=20))]
        top_k: u8,
    },
    /// Read the normalized schema and retained API contract of an exact saved tool.
    Describe {
        #[arg(long)]
        name: String,
        tool: String,
        /// Emit the bounded agent view instead of the complete normalized contract.
        #[arg(long)]
        model_view: bool,
        #[arg(long, requires = "model_view", default_value_t = 0)]
        parameter_offset: u32,
    },
}

#[must_use]
pub fn run(command: ToolsCommand) -> ExitCode {
    if ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed)).is_err() {
        eprintln!(
            "{}",
            json!({"ok": false, "error": "interrupt_handler_unavailable"})
        );
        return ExitCode::FAILURE;
    }
    let result = match command {
        ToolsCommand::Connect {
            name,
            base_url,
            allow_get,
            bearer_env,
            bearer,
            token_stdin,
        } => {
            if allow_get {
                crate::http_read::connect(&name, base_url, bearer_env, token_stdin, bearer)
            } else {
                Err("http_get_consent_required")
            }
        }
        command => request(command).and_then(|request| invoke(&request)),
    };
    match result {
        Ok(response) => {
            // JSON encoding keeps terminal controls escaped, including across chunks.
            println!("{response}");
            if response.get("ok") == Some(&Value::Bool(true)) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(code) => {
            eprintln!("{}", json!({"ok": false, "error": code}));
            ExitCode::FAILURE
        }
    }
}

fn request(command: ToolsCommand) -> Result<Value, &'static str> {
    let mut value = match command {
        ToolsCommand::Import { name, source } => {
            if source.len() > 16 {
                return Err("too_many_sources");
            }
            let sources: Result<Vec<_>, _> = source
                .iter()
                .map(|source| {
                    if source.len() > 4096 {
                        return Err("invalid_source");
                    }
                    if source.starts_with("https://") {
                        let url = url::Url::parse(source).map_err(|_| "invalid_source")?;
                        if !url.username().is_empty()
                            || url.password().is_some()
                            || url.query().is_some()
                            || url.fragment().is_some()
                        {
                            return Err("credential_or_query_in_source_url");
                        }
                        Ok(source.clone())
                    } else {
                        std::fs::canonicalize(source)
                            .map_err(|_| "source_unavailable")?
                            .into_os_string()
                            .into_string()
                            .map_err(|_| "invalid_source")
                    }
                })
                .collect();
            json!({"operation": "import", "name": name, "sources": sources?})
        }
        ToolsCommand::Connect { .. } => return Err("unsupported_transport_operation"),
        ToolsCommand::List => json!({"operation": "list"}),
        ToolsCommand::Search { name, query, top_k } => {
            if query.trim().is_empty() || query.len() > 4096 {
                return Err("invalid_query");
            }
            json!({"operation": "search", "name": name, "query": query, "top_k": top_k})
        }
        ToolsCommand::Describe {
            name,
            tool,
            model_view,
            parameter_offset,
        } => {
            if tool.is_empty() || tool.len() > 1024 {
                return Err("invalid_tool");
            }
            json!({"operation": if model_view {"describe_view"} else {"describe"}, "name": name, "tool": tool, "parameter_offset":parameter_offset})
        }
    };
    if let Some(name) = value.get("name").and_then(Value::as_str)
        && (name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        return Err("invalid_collection_name");
    }
    let state_root = crate::run_layout::discover_state_root().map_err(|_| "invalid_state_home")?;
    crate::run_layout::ensure_private_state_root(&state_root).map_err(|_| "invalid_state_home")?;
    let root = state_root.join("tool-collections");
    value["root"] = json!(root.to_str().ok_or("invalid_state_home")?);
    Ok(value)
}

fn runtime_command(offline: bool) -> Command {
    let mut command = Command::new("uv");
    if offline {
        command.arg("--offline");
    }
    command
        .args([
            "run",
            "--no-project",
            "--no-config",
            "--no-env-file",
            "--isolated",
            "--python",
            "3.12",
            "--with",
            "graph-tool-call==0.47.0",
            "--",
            "python",
            "-I",
            "-c",
            WORKER,
        ])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Do not forward API keys, Python injection paths or package-index credentials.
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "SYSTEMROOT",
        "TMP",
        "TEMP",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}

pub(crate) fn invoke(request: &Value) -> Result<Value, &'static str> {
    invoke_worker(request, runtime_command(false), TIMEOUT)
}

pub(crate) fn invoke_offline(request: &Value) -> Result<Value, &'static str> {
    invoke_worker(request, runtime_command(true), TIMEOUT)
}

fn invoke_worker(
    request: &Value,
    mut command: Command,
    timeout: Duration,
) -> Result<Value, &'static str> {
    let encoded = request.to_string().into_bytes();
    if encoded.len() > 65536 {
        return Err("request_size_limit");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(JobObject);
    let mut child = wrapped
        .spawn()
        .map_err(|_| "tool_runtime_unavailable_install_uv")?;
    let started = Instant::now();
    let Some(mut input) = child.stdin().take() else {
        stop(child.as_mut());
        return Err("tool_runtime_io");
    };
    let (input_sender, input_receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = input.write_all(&encoded);
        drop(input);
        let _ = input_sender.send(result);
    });
    let Some(output) = child.stdout().take() else {
        stop(child.as_mut());
        return Err("tool_runtime_io");
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.take(MAX_RESPONSE + 1).read_to_end(&mut bytes);
        let _ = sender.send((result, bytes));
    });
    let (read_result, bytes) = loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            stop(child.as_mut());
            return Err("tool_runtime_interrupted");
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) if started.elapsed() < timeout => {}
            _ => {
                stop(child.as_mut());
                return Err("tool_runtime_timeout");
            }
        }
    };
    if read_result.is_err() || bytes.len() as u64 > MAX_RESPONSE {
        stop(child.as_mut());
        return Err("tool_runtime_output_limit");
    }
    if !matches!(
        input_receiver.recv_timeout(Duration::from_secs(1)),
        Ok(Ok(()))
    ) {
        stop(child.as_mut());
        return Err("tool_runtime_io");
    }
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            stop(child.as_mut());
            return Err("tool_runtime_interrupted");
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                // The leader can exit while a descendant with closed pipes remains alive.
                let _ = child.start_kill();
                break;
            }
            Ok(Some(_)) | Err(_) => {
                stop(child.as_mut());
                return Err("tool_runtime_failed");
            }
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                stop(child.as_mut());
                return Err("tool_runtime_timeout");
            }
        }
    }
    serde_json::from_slice(&bytes).map_err(|_| "tool_runtime_invalid_response")
}

fn stop(child: &mut dyn ChildWrapper) {
    let _ = child.start_kill();
    let deadline = Instant::now() + Duration::from_secs(2);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_bounded_even_before_stdin_is_consumed() {
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import time; time.sleep(10)"]);
        let started = Instant::now();
        assert_eq!(
            invoke_worker(
                &json!({"data": "x".repeat(32000)}),
                command,
                Duration::from_millis(100)
            ),
            Err("tool_runtime_timeout")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn exited_worker_does_not_leave_its_descendants_running() {
        for exit_code in [0, 2] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("pid");
            let mut command = Command::new("/usr/bin/python3");
            command.args(["-c", "import sys, subprocess; from pathlib import Path; sys.stdin.read(); p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); Path(sys.argv[1]).write_text(str(p.pid)); print('{\"ok\":true}'); sys.exit(int(sys.argv[2]))"]);
            command.arg(&path).arg(exit_code.to_string());
            let result = invoke_worker(&json!({}), command, Duration::from_secs(2));
            assert_eq!(result.is_ok(), exit_code == 0);
            let pid = std::fs::read_to_string(path).unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let process = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                if process.is_err() || process.unwrap().split_whitespace().nth(2) == Some("Z") {
                    break;
                }
                assert!(Instant::now() < deadline, "worker descendant survived");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
