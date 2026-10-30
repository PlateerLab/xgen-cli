#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::process::Command;
use tempfile::tempdir;

const RUN_ID: &str = "run-0123456789abcdef0123456789abcdef";

#[test]
#[allow(clippy::unnecessary_debug_formatting)]
fn every_state_directory_reports_its_permissions_without_repairing_them() {
    for (relative, mode) in [
        ("", 0o775),
        ("runs", 0o755),
        ("runs/run-0123456789abcdef0123456789abcdef", 0o770),
    ] {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("state with spaces");
        let run = root.join("runs").join(RUN_ID);
        fs::create_dir_all(&run).unwrap();
        for path in [&root, &root.join("runs"), &run] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let rejected = if relative.is_empty() {
            root.clone()
        } else {
            root.join(relative)
        };
        fs::set_permissions(&rejected, fs::Permissions::from_mode(mode)).unwrap();
        for command in ["usage", "recover"] {
            let output = Command::new(env!("CARGO_BIN_EXE_xgen"))
                .env("XGEN_STATE_HOME", &root)
                .args([command, RUN_ID])
                .output()
                .unwrap();
            let error = String::from_utf8(output.stderr).unwrap();
            assert_eq!(output.status.code(), Some(64), "{error}");
            assert!(output.stdout.is_empty());
            assert!(
                error.contains("XGEN_ERROR code=state_directory_permissions"),
                "{error}"
            );
            assert!(
                error.contains(&format!("state_directory={rejected:?}")),
                "{error}"
            );
            assert!(
                error.contains(&format!("permissions=0{mode:03o}")),
                "{error}"
            );
            assert!(error.contains("chmod go-rwx -- PATH"));
            assert_eq!(
                fs::metadata(&rejected).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert!(!run.join("run.sqlite3").exists());
        }
        fs::set_permissions(&rejected, fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_xgen"))
            .env("XGEN_STATE_HOME", &root)
            .args(["usage", RUN_ID])
            .output()
            .unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("run_integrity_failure"), "{error}");
        assert!(!error.contains("state_directory_permissions"));
        assert!(!run.join("run.sqlite3").exists());
    }
}

#[test]
fn symlinks_remain_generic_errors_and_never_suggest_permission_repairs() {
    let fixture = tempdir().unwrap();
    let target = fixture.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o775)).unwrap();
    let root = fixture.path().join("linked-state");
    symlink(&target, &root).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xgen"))
        .env("XGEN_STATE_HOME", &root)
        .args(["usage", RUN_ID])
        .output()
        .unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("configuration_mismatch"), "{error}");
    assert!(!error.contains("state_directory="));
    assert!(!error.contains("chmod"));
    assert_eq!(
        fs::metadata(target).unwrap().permissions().mode() & 0o777,
        0o775
    );
}
