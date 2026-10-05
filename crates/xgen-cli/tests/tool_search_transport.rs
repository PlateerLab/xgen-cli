//! Host-side transport checks without package downloads or model calls.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn fake_runtime(script: &str) -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("uv");
    fs::write(&path, format!("#!/usr/bin/python3\n{script}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

fn command(directory: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xgen"));
    command
        .args(["tools", "list"])
        .env("PATH", directory.path())
        .env("XGEN_STATE_HOME", directory.path().join("state"));
    command
}

#[test]
fn credentials_and_python_injection_are_not_forwarded() {
    let directory = fake_runtime(
        "import sys, json, os\nsys.stdin.read()\nprint(json.dumps({'ok': True, 'credential': os.environ.get('XGEN_OPENAI_API_KEY'), 'python_path': os.environ.get('PYTHONPATH')}))",
    );
    let output = command(&directory)
        .env("XGEN_OPENAI_API_KEY", "test-private-marker")
        .env("PYTHONPATH", "/test/injection")
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response["credential"].is_null());
    assert!(response["python_path"].is_null());
}

#[test]
fn runtime_errors_do_not_print_stderr() {
    let directory = fake_runtime(
        "import sys\nsys.stdin.read()\nsys.stderr.write('test-private-marker')\nsys.exit(2)",
    );
    let output = command(&directory).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let response: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(response["error"], "tool_runtime_failed");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-private-marker"));
}

#[test]
fn oversized_output_is_bounded_and_rejected() {
    let directory = fake_runtime(
        "import sys\nsys.stdin.read()\nsys.stdout.buffer.write(b'x' * (16 * 1024 * 1024 + 2))",
    );
    let output = command(&directory).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let response: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(response["error"], "tool_runtime_output_limit");
}

#[test]
fn query_and_url_credentials_are_rejected_before_runtime() {
    let directory = tempfile::tempdir().unwrap();
    for url in [
        "https://user:password@example.org/openapi.json",
        "https://example.org/openapi.json?token=private",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_xgen"))
            .args(["tools", "import", "--name", "sample", "--source", url])
            .env("PATH", directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let response: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(response["error"], "credential_or_query_in_source_url");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private"));
    }
}

#[test]
fn interrupt_stops_a_running_worker() {
    let directory = fake_runtime(
        "import sys, json, time\nfrom pathlib import Path\nr=json.load(sys.stdin)\n(Path(r['root']).parents[1] / 'started').write_text('ready')\ntime.sleep(30)",
    );
    let mut child = command(&directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = directory.path().join("started");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !started.exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fake worker did not start");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(response["error"], "tool_runtime_interrupted");
}
