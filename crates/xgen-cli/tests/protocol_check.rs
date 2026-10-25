use std::process::Command;

#[cfg(unix)]
use std::process::Stdio;

use tempfile::tempdir;

fn xgen() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgen"))
}

#[test]
fn protocol_check_succeeds_and_reports_conformance_scope() {
    let output = xgen()
        .args(["protocol", "check"])
        .output()
        .expect("xgen should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("XGEN protocol v0.1: PASS"));
    assert!(stdout.contains("schemas: 9"));
    assert!(stdout.contains("fixtures: 34 (24 valid, 10 invalid)"));
    assert!(stdout.contains("reference resolution: bundled/offline"));
}

#[test]
fn version_is_available_without_running_protocol_checks() {
    let output = xgen()
        .arg("--version")
        .output()
        .expect("xgen should execute");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("stdout should be UTF-8")
            .trim(),
        format!("xgen {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn licenses_are_embedded_and_available_offline() {
    let output = xgen()
        .arg("licenses")
        .output()
        .expect("xgen should execute");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("===== XGEN project license ====="));
    assert!(stdout.contains("===== Cargo dependency notices ====="));
    assert!(stdout.contains("XGEN CLI Third-Party License Notices"));
    assert!(stdout.contains("===== Rust standard library notices ====="));
    assert!(stdout.contains("Copyright notices for The Rust Standard Library"));
    assert!(stdout.contains("===== musl C runtime notices ====="));
    assert!(stdout.contains("Copyright © 2005-2020 Rich Felker, et al."));
    assert!(
        stdout.contains("musl as a whole is licensed under the following standard MIT license")
    );
    assert!(stdout.contains("===== LLVM libunwind notices ====="));
    assert!(stdout.contains("University of Illinois/NCSA"));
    assert!(stdout.contains("Open Source License"));
    assert!(stdout.contains("Copyright (c) 2009-2014 by the contributors listed in CREDITS.TXT"));
}

#[cfg(unix)]
#[test]
fn licenses_treat_an_early_reader_close_as_success() {
    let mut child = xgen()
        .arg("licenses")
        .stdout(Stdio::piped())
        .spawn()
        .expect("xgen should execute");
    drop(child.stdout.take().expect("stdout should be piped"));

    let status = child.wait().expect("xgen should exit");
    assert!(status.success());
}

#[test]
fn unknown_command_returns_nonzero_exit() {
    let output = xgen()
        .arg("unknown-command")
        .output()
        .expect("xgen should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("unrecognized subcommand"));
}

#[test]
fn environment_backed_run_arguments_reach_explicit_egress_consent_gate() {
    let workspace = tempdir().expect("workspace should exist");
    let output = xgen()
        .current_dir(workspace.path())
        .env("XGEN_OPENAI_BASE_URL", "http://127.0.0.1:18000/v1")
        .env("XGEN_OPENAI_MODEL", "test-model")
        .env_remove("XGEN_OPENAI_TOKENIZER")
        .env_remove("XGEN_OPENAI_API_KEY")
        .args(["run", "--allow-file", "README.md", "test goal"])
        .output()
        .expect("xgen should parse environment-backed model settings");

    assert_eq!(output.status.code(), Some(10));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("reason=remote_model_egress_consent_required"));
}

#[test]
fn run_requires_an_explicit_file_or_directory_scope() {
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", "http://127.0.0.1:18000/v1")
        .env("XGEN_OPENAI_MODEL", "test-model")
        .args(["run", "test goal"])
        .output()
        .expect("xgen should reject a scope-free run at argument parsing");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("--allow-file"));
    assert!(stderr.contains("--allow-dir"));
}

#[test]
fn run_help_names_environment_contract_without_exposing_values() {
    let sentinel = "SENSITIVE-MODEL-SETTING-MUST-NOT-APPEAR";
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", sentinel)
        .env("XGEN_OPENAI_MODEL", sentinel)
        .env("XGEN_OPENAI_TOKENIZER", sentinel)
        .args(["run", "--help"])
        .output()
        .expect("xgen help should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("XGEN_OPENAI_BASE_URL"));
    assert!(stdout.contains("XGEN_OPENAI_MODEL"));
    assert!(stdout.contains("XGEN_OPENAI_TOKENIZER"));
    assert!(stdout.contains("XGEN_OPENAI_API_KEY"));
    assert!(stdout.contains("--allow-file"));
    assert!(stdout.contains("--allow-dir"));
    assert!(stdout.contains("--allow-write"));
    assert!(stdout.contains("--allow-executable"));
    assert!(stdout.contains("--allow-execute"));
    assert!(!stdout.contains(sentinel));
}

#[test]
fn execute_approval_without_an_executable_catalog_is_rejected() {
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", "http://127.0.0.1:18000/v1")
        .env("XGEN_OPENAI_MODEL", "test-model")
        .args([
            "run",
            "--allow-file",
            "README.md",
            "--allow-execute",
            "test goal",
        ])
        .output()
        .expect("xgen should reject execution approval without a catalog");

    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("code=configuration_mismatch"));
    // The shared configuration code says nothing on its own, and the flag was
    // just typed, so the failure names which half is missing and both ways out.
    assert!(stderr.contains("configuration=executable_catalog"));
    assert!(stderr.contains("--allow-executable ID=ABSOLUTE_PATH"));
    assert!(stderr.contains("--allow-path-executables"));
}

/// A directory holding one executable named after a discovered developer tool.
#[cfg(unix)]
fn path_with_a_developer_tool(directory: &std::path::Path) -> &std::path::Path {
    use std::os::unix::fs::PermissionsExt as _;

    let tool = directory.join("git");
    std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

/// `--allow-path-executables` is the catalog `--allow-execute` needs.
///
/// Egress consent is checked after the catalog, so the pause that asks for it
/// is proof the catalog was accepted; nothing is sent and no model is involved.
#[cfg(unix)]
#[test]
fn path_discovery_supplies_the_executable_catalog() {
    let directory = tempdir().expect("temporary directory");
    let workspace = tempdir().expect("temporary workspace");
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", "http://127.0.0.1:18000/v1")
        .env("XGEN_OPENAI_MODEL", "test-model")
        .env("PATH", path_with_a_developer_tool(directory.path()))
        .args(["run", "--workspace"])
        .arg(workspace.path())
        .args([
            "--allow-dir",
            ".",
            "--allow-execute",
            "--allow-path-executables",
            "test goal",
        ])
        .output()
        .expect("xgen should accept a discovered catalog");

    assert_eq!(output.status.code(), Some(10));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("reason=remote_model_egress_consent_required"));
    assert!(!stderr.contains("configuration_mismatch"));
}

/// Asking for discovery that finds nothing is still an empty catalog.
#[cfg(unix)]
#[test]
fn path_discovery_that_finds_nothing_says_so() {
    let empty = tempdir().expect("temporary directory");
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", "http://127.0.0.1:18000/v1")
        .env("XGEN_OPENAI_MODEL", "test-model")
        .env("PATH", empty.path())
        .args([
            "run",
            "--allow-file",
            "README.md",
            "--allow-execute",
            "--allow-path-executables",
            "test goal",
        ])
        .output()
        .expect("xgen should reject an empty discovered catalog");

    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("configuration=executable_catalog"));
    assert!(stderr.contains("found no known developer executable"));
}

#[test]
fn model_check_help_names_secret_environment_contract_without_exposing_values() {
    let sentinel = "SENSITIVE-MODEL-CHECK-SETTING-MUST-NOT-APPEAR";
    let output = xgen()
        .env("XGEN_OPENAI_BASE_URL", sentinel)
        .env("XGEN_OPENAI_MODEL", sentinel)
        .env("XGEN_OPENAI_TOKENIZER", sentinel)
        .env("XGEN_OPENAI_API_KEY", sentinel)
        .args(["model", "check", "--help"])
        .output()
        .expect("xgen model check help should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("XGEN_OPENAI_BASE_URL"));
    assert!(stdout.contains("XGEN_OPENAI_MODEL"));
    assert!(stdout.contains("XGEN_OPENAI_TOKENIZER"));
    assert!(stdout.contains("XGEN_OPENAI_API_KEY"));
    assert!(!stdout.contains(sentinel));
}
