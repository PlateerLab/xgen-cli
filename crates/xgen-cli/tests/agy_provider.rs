#![cfg(target_os = "linux")]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use tempfile::{TempDir, tempdir};

struct Fixture {
    root: TempDir,
}
impl Fixture {
    fn new(mode: &str, timeout: u64) -> Self {
        let root = tempdir().unwrap();
        for name in ["home", "state", "config", "workspace"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(root.path().join("workspace/note.txt"), "fixture\n").unwrap();
        fs::write(
            root.path().join("workspace/tests.py"),
            "from pathlib import Path\nassert Path('result.txt').read_text()=='ready\\n'\n",
        )
        .unwrap();
        let executable = root.path().join("fake-agy");
        let prefix = format!(
            "#!/usr/bin/env python3\nMODE={}\nMARKER={}\n",
            serde_json::to_string(mode).unwrap(),
            serde_json::to_string(root.path().join("leak").to_str().unwrap()).unwrap()
        );
        fs::write(&executable, prefix + include_str!("fixtures/agy_fake.py")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self { root };
        let result = fixture
            .command()
            .args([
                "model",
                "setup",
                "--provider",
                "agy",
                "--name",
                "fixture",
                "--agy-executable",
            ])
            .arg(executable)
            .args(["--inference-timeout", &timeout.to_string()])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fixture
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_xgen"));
        cmd.env("HOME", self.root.path().join("home"))
            .env("XGEN_CONFIG_HOME", self.root.path().join("config"))
            .env("XGEN_STATE_HOME", self.root.path().join("state"))
            .env("XGEN_OPENAI_API_KEY", "parent-secret-must-not-reach-agy")
            .env("XGEN_AGY_TEST_SECRET", "private-test-sentinel")
            .env("SSH_CONNECTION", "fixture")
            .env("SSH_CLIENT", "fixture")
            .env_remove("XGEN_OPENAI_BASE_URL")
            .env_remove("XGEN_OPENAI_MODEL")
            .env_remove("XGEN_OPENAI_TOKENIZER")
            .env_remove("XGEN_OPENAI_INFERENCE_TIMEOUT")
            .env_remove("XGEN_OPENAI_MAX_OUTPUT_TOKENS")
            .env_remove("XGEN_OPENAI_RESPONSE_FORMAT")
            .env_remove("XGEN_OPENAI_THINKING")
            .env_remove("XGEN_OPENAI_PARAMETER_STYLE");
        cmd
    }
    fn run(&self, write: bool) -> Output {
        let mut cmd = self.command();
        cmd.args([
            "run",
            if write {
                "Create result.txt containing ready and newline, then verify using tests.py."
            } else {
                "Read note.txt and answer."
            },
            "--workspace",
        ])
        .arg(self.root.path().join("workspace"))
        .args([
            "--allow-dir",
            ".",
            "--allow-read",
            "--allow-remote-model-egress",
        ]);
        if write {
            cmd.args([
                "--allow-write",
                "--allow-executable",
                "python3=/usr/bin/python3",
            ]);
        }
        cmd.output().unwrap()
    }
    fn run_id(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .find_map(|v| v.strip_prefix("run_id="))
            .unwrap()
            .to_owned()
    }
    fn resume(&self, id: &str, execute: bool) -> Output {
        let mut cmd = self.command();
        cmd.args(["resume", id, "--workspace"])
            .arg(self.root.path().join("workspace"))
            .args([
                "--allow-dir",
                ".",
                "--allow-read",
                "--allow-write",
                "--allow-executable",
                "python3=/usr/bin/python3",
                "--allow-remote-model-egress",
            ]);
        if execute {
            cmd.arg("--allow-execute");
        }
        cmd.output().unwrap()
    }
}

#[test]
fn saved_agy_profile_runs_without_api_key_and_completed_run_replays_offline() {
    let f = Fixture::new("normal", 5);
    let probe = f
        .command()
        .args(["model", "check", "--compatibility"])
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let out = f.run(false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"done");
    let id = Fixture::run_id(&out);
    let db = rusqlite::Connection::open(
        f.root
            .path()
            .join("state/runs")
            .join(&id)
            .join("usage.sqlite3"),
    )
    .unwrap();
    let records: Vec<String> = db
        .prepare("SELECT record_json FROM observations")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(records.len(), 2);
    for record in records {
        let value: serde_json::Value = serde_json::from_str(&record).unwrap();
        assert_eq!(value["usage"]["inputTokens"], 250);
        assert_eq!(value["usage"]["cachedInputTokens"], 150);
        assert_eq!(value["usage"]["reasoningTokens"], 5);
    }
    drop(db);
    fs::rename(
        f.root.path().join("workspace"),
        f.root.path().join("moved-workspace"),
    )
    .unwrap();
    fs::remove_file(f.root.path().join("fake-agy")).unwrap();
    let out = f.command().args(["resume", &id]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"done");
}

#[test]
fn agy_execution_approval_resume_and_executable_drift() {
    let f = Fixture::new("normal", 5);
    let out = f.run(true);
    assert_eq!(
        out.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("execute_approval_required"));
    assert_eq!(
        fs::read(f.root.path().join("workspace/result.txt")).unwrap(),
        b"ready\n"
    );
    let id = Fixture::run_id(&out);
    let out = f.resume(&id, true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let g = Fixture::new("normal", 5);
    let id = Fixture::run_id(&g.run(true));
    let path = g.root.path().join("fake-agy");
    let mut body = fs::read(&path).unwrap();
    body.extend_from_slice(b"\n# changed\n");
    fs::write(path, body).unwrap();
    let out = g.resume(&id, true);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("configuration_mismatch"));
}

#[test]
fn native_activity_timeout_and_output_limits_fail_without_late_child_effects() {
    for mode in ["native", "timeout", "flood", "duplicate", "settings"] {
        let f = Fixture::new(mode, 1);
        let started = Instant::now();
        let out = f.run(false);
        assert!(!out.status.success(), "{mode}");
        assert!(started.elapsed() < Duration::from_secs(8), "{mode}");
        if mode == "native" {
            assert!(String::from_utf8_lossy(&out.stderr).contains("native_tool_observed"));
        }
        std::thread::sleep(Duration::from_millis(2200));
        assert!(
            !f.root.path().join("leak").exists(),
            "{mode} escaped child cleanup"
        );
    }
}

#[test]
fn sigint_stops_owned_agy_process_group() {
    let f = Fixture::new("timeout", 20);
    let mut cmd = f.command();
    let mut child = cmd
        .args(["run", "Read note.txt", "--workspace"])
        .arg(f.root.path().join("workspace"))
        .args([
            "--allow-dir",
            ".",
            "--allow-read",
            "--allow-remote-model-egress",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !f.root.path().join("leak.pid").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(!child.wait().unwrap().success());
    let private_workspace = fs::read_to_string(f.root.path().join("leak.cwd")).unwrap();
    assert!(!Path::new(&private_workspace).parent().unwrap().exists());
    std::thread::sleep(Duration::from_millis(2200));
    assert!(!f.root.path().join("leak").exists());
}

#[test]
fn profile_descriptor_binds_policy_and_survives_missing_binary_for_listing() {
    let f = Fixture::new("normal", 5);
    let spec = xgen_cli::agy::Spec::create(&f.root.path().join("fake-agy")).unwrap();
    let uri = spec.uri().unwrap();
    assert_eq!(
        xgen_cli::agy::Spec::parse(&uri)
            .unwrap()
            .endpoint()
            .unwrap(),
        spec.endpoint().unwrap()
    );
    assert!(xgen_cli::agy::Spec::parse("agy:///tmp/tool?sha256=bad").is_err());
    fs::remove_file(f.root.path().join("fake-agy")).unwrap();
    assert!(
        f.command()
            .args(["model", "list"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(Path::new(&f.root.path().join("config/model-profiles.json")).exists());
}
