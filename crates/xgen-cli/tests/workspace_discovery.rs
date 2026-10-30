use std::fs;
use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use xgen_local_store::{RunStore, SqliteRunStore};
use xgen_workgraph::{RunEventBody, StepStatus};

const MODEL: &str = "test-workspace-model";
const TOKENIZER: &str = "test-workspace-tokenizer";
const NEEDLE: &str = "XGEN_WORKSPACE_DISCOVERY_NEEDLE";
const COMPLETION: &str = "workspace discovery completed";
const TEST_TIMEOUT: Duration = Duration::from_secs(60);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(180);

#[test]
fn invocation_diagnostics_distinguish_schema_and_resource_failures_without_values() {
    let cases = [
        (
            json!({"path":"DECISION.json","content":"PRIVATE"}),
            "schema_required",
            "expectedDigest",
        ),
        (
            json!({"path":"DECISION.json","content":{"SECRET":"PRIVATE"},"expectedDigest":null}),
            "schema_type",
            "content",
        ),
        (
            json!({"path":"DECISION.json","content":"PRIVATE","expectedDigest":null,"SECRET":"PRIVATE"}),
            "schema_additional_property",
            "other",
        ),
        (
            json!({"path":"","content":"PRIVATE","expectedDigest":null}),
            "schema_min_length",
            "path",
        ),
        (
            json!({"path":"../SECRET","content":"PRIVATE","expectedDigest":null}),
            "resource_resolution",
            "other",
        ),
        (
            json!({"path":"DECISION.json","content":"PRIVATE","expectedDigest":"SECRET"}),
            "schema_one_of",
            "expectedDigest",
        ),
    ];
    for (arguments, category, field) in cases {
        let fixture = tempdir().unwrap();
        let state_root = fixture.path().join("state");
        let workspace = fixture.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        // Each rejected proposal is retried up to MAX_REJECTED_RESPONSE_RETRIES times (ADR-0061).
        let server = SequentialServer::spawn_responses(
            (0..=xgen_cli::MAX_REJECTED_RESPONSE_RETRIES)
                .map(|_| {
                    plan_response(
                        "write",
                        "Write fixture",
                        "xgeny.fs/write-atomic",
                        &arguments,
                    )
                })
                .collect(),
        );
        let output = bounded_output(xgen(&state_root).args([
            "run",
            "--workspace",
            path_text(&workspace),
            "--base-url",
            &server.base_url,
            "--model",
            MODEL,
            "--tokenizer",
            TOKENIZER,
            "--allow-dir",
            ".",
            "--allow-write",
            "--allow-remote-model-egress",
            "Write a fixture.",
        ]))
        .unwrap();
        let text = stderr(&output);
        assert_eq!(output.status.code(), Some(20), "{text}");
        let run_id = extract_run_id(&text);
        assert!(text.contains(&format!(
            "XGEN_REJECTED run_id={run_id} reason=proposal_rejected.invocation_invalid"
        )));
        assert!(text.contains(&format!("XGEN_INVOCATION_DIAGNOSTIC run_id={run_id} version=1 category={category} field={field}")), "{text}");
        for secret in ["SECRET", "PRIVATE", path_text(&workspace)] {
            assert!(!text.contains(secret));
        }
        assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
        assert!(!fixture.path().join("SECRET").exists());
        let db = state_root.join("runs").join(run_id).join("run.sqlite3");
        let store = SqliteRunStore::open_existing(db).unwrap();
        assert!(store.load_execution_receipts().unwrap().is_empty());
        for _ in 0..=xgen_cli::MAX_REJECTED_RESPONSE_RETRIES {
            server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        server.handle.join().unwrap();
        assert!(server.requests.try_recv().is_err());
    }
}

struct SequentialServer {
    base_url: String,
    requests: Receiver<Vec<u8>>,
    handle: thread::JoinHandle<()>,
}

impl SequentialServer {
    fn spawn() -> Self {
        Self::spawn_responses(vec![
            plan_response(
                "list_workspace",
                "List the workspace root",
                "xgeny.fs/list-directory",
                &json!({"path": "."}),
            ),
            plan_response(
                "search_workspace",
                "Find the requested marker",
                "xgeny.fs/search-text",
                &json!({"path": ".", "query": NEEDLE}),
            ),
            plan_response(
                "stat_match",
                "Inspect the matching file",
                "xgeny.fs/stat",
                &json!({"path": "src/lib.rs"}),
            ),
            plan_response(
                "read_match",
                "Read the matching file",
                "xgeny.fs/read-text",
                &json!({"path": "src/lib.rs"}),
            ),
            completion_response(),
        ])
    }

    fn spawn_responses(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("listener address should resolve");
        let (sender, requests) = mpsc::channel();
        let handle = thread::spawn(move || {
            for response in responses {
                let mut stream =
                    accept_with_timeout(&listener).expect("each planned model turn should connect");
                let request = read_http_request(&mut stream);
                if sender.send(request).is_err() {
                    return;
                }
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                stream
                    .write_all(headers.as_bytes())
                    .and_then(|()| stream.write_all(&response))
                    .expect("provider response should write");
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            handle,
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn process_execution_requires_separate_approval_and_resumes_without_replay() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace should create");
    fs::write(workspace.join("README.md"), "process integration fixture\n")
        .expect("workspace fixture should write");
    let xgen_binary = Path::new(env!("CARGO_BIN_EXE_xgen"));
    let executable_spec = format!("xgen={}", path_text(xgen_binary));
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "check_protocol",
            "Run the bundled protocol checks without a shell",
            "xgeny.process/execute",
            &json!({
                "executable": "xgen",
                "args": ["protocol", "check"],
                "cwd": ".",
                "env": {},
                "timeoutMs": 30000,
                "maxOutputBytes": 32768
            }),
        ),
        completion_response(),
    ]);

    let first = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-remote-model-egress",
        "Run the protocol check and report the result.",
    ]))
    .expect("unapproved process should pause");
    assert_eq!(first.status.code(), Some(10), "{}", stderr(&first));
    assert!(stderr(&first).contains("reason=execute_approval_required"));
    let run_id = extract_run_id(&stderr(&first));

    let first_request = server
        .requests
        .recv_timeout(TEST_TIMEOUT)
        .expect("initial planning request should arrive");
    let first_context = planning_context(&first_request);
    assert!(capability_ids(&first_context).contains(&"xgeny.process/execute"));
    let constraints = first_context["planningConstraints"]
        .as_array()
        .expect("filesystem and process constraints should be present");
    assert_eq!(constraints.len(), 2);
    assert_eq!(constraints[1]["constraintId"], "process.executable-catalog");
    assert!(
        constraints[1]["description"]
            .as_str()
            .unwrap()
            .contains("xgen")
    );
    assert!(
        !constraints[1]["description"]
            .as_str()
            .unwrap()
            .contains(path_text(xgen_binary))
    );

    let different_executable = std::env::current_exe().expect("test executable should resolve");
    let different_specification = format!("xgen={}", path_text(&different_executable));
    let mismatch = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-executable",
        &different_specification,
        "--allow-execute",
    ]))
    .expect("changed executable catalog should fail closed");
    assert_eq!(mismatch.status.code(), Some(64), "{}", stderr(&mismatch));
    assert!(stderr(&mismatch).contains("code=configuration_mismatch"));

    let local = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-execute",
    ]))
    .expect("approved process should execute without model access");
    assert_eq!(local.status.code(), Some(10), "{}", stderr(&local));
    assert!(stderr(&local).contains("reason=remote_model_egress_consent_required"));

    let completion = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-execute",
        "--allow-remote-model-egress",
    ]))
    .expect("remote continuation should complete");
    assert_eq!(completion.status.code(), Some(0), "{}", stderr(&completion));
    assert_eq!(String::from_utf8_lossy(&completion.stdout), COMPLETION);

    let completion_request = server
        .requests
        .recv_timeout(TEST_TIMEOUT)
        .expect("completion request should arrive");
    server.handle.join().expect("provider server should finish");
    let completion_context = planning_context(&completion_request);
    let output = tool_output(&completion_context, "xgeny.process/execute");
    assert_eq!(output["outcome"], "exited");
    assert_eq!(output["success"], true);
    assert!(
        output["stdout"]
            .as_str()
            .expect("process stdout should be text")
            .contains("XGEN protocol v0.1: PASS")
    );

    let database = state_root.join("runs").join(&run_id).join("run.sqlite3");
    let store = SqliteRunStore::open_existing(database).expect("Run store should reopen");
    let receipts = store
        .load_execution_receipts()
        .expect("execution receipts should load");
    assert_eq!(
        receipts.len(),
        1,
        "completed process must not replay on resume"
    );
    assert_eq!(
        receipts[0].capability.capability_id,
        "xgeny.process/execute"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn process_outcome_commit_failure_recovers_without_reexecution() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    let marker = fixture.path().join("process-executions.log");
    fs::create_dir(&workspace).expect("workspace should create");
    let test_binary = std::env::current_exe().expect("test executable should resolve");
    let executable_spec = format!("test-helper={}", path_text(&test_binary));
    let server = SequentialServer::spawn_responses(vec![plan_response(
        "faulted_process",
        "Run the test helper once without a shell",
        "xgeny.process/execute",
        &json!({
            "executable": "test-helper",
            "args": [
                "process_no_replay_child",
                "--exact",
                "--nocapture",
                "--test-threads=1"
            ],
            "cwd": ".",
            "env": {
                "XGEN_PROCESS_NO_REPLAY_MARKER": path_text(&marker)
            },
            "timeoutMs": 30000,
            "maxOutputBytes": 32768
        }),
    )]);

    let planned = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-remote-model-egress",
        "Run the test helper exactly once.",
    ]))
    .expect("unapproved process should pause");
    assert_eq!(planned.status.code(), Some(10), "{}", stderr(&planned));
    assert!(stderr(&planned).contains("reason=execute_approval_required"));
    let run_id = extract_run_id(&stderr(&planned));
    server
        .requests
        .recv_timeout(TEST_TIMEOUT)
        .expect("planning request should arrive");
    server.handle.join().expect("provider server should finish");

    let database = state_root.join("runs").join(&run_id).join("run.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("fault fixture should open");
    connection
        .execute_batch(
            r"
            CREATE TRIGGER test_abort_process_effect_succeeded
            BEFORE INSERT ON run_events
            WHEN json_extract(CAST(NEW.event_json AS TEXT), '$.body.type') = 'effect_succeeded'
            BEGIN
                SELECT RAISE(ABORT, 'injected process outcome commit failure');
            END;
            ",
        )
        .expect("outcome fault trigger should install");
    drop(connection);

    let faulted = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-execute",
    ]))
    .expect("faulted process should return");
    assert_eq!(faulted.status.code(), Some(30), "{}", stderr(&faulted));
    assert!(stderr(&faulted).contains("reason=effect_outcome_unknown"));
    assert!(!stderr(&faulted).contains("injected process outcome commit failure"));
    assert_eq!(process_execution_count(&marker), 1);

    let executing_store =
        SqliteRunStore::open_existing(&database).expect("Executing store should reopen");
    let executing = executing_store
        .load()
        .expect("Executing snapshot should load")
        .expect("Executing snapshot should exist");
    assert!(
        executing
            .state
            .steps
            .values()
            .any(|step| step.status == StepStatus::Executing)
    );
    assert_eq!(
        executing
            .records
            .iter()
            .filter(|record| matches!(
                record.event.body,
                RunEventBody::EffectExecutionStarted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        executing
            .records
            .iter()
            .filter(|record| matches!(record.event.body, RunEventBody::EffectSucceeded { .. }))
            .count(),
        0
    );
    let effect_id = executing
        .state
        .steps
        .values()
        .find_map(|step| step.intent.as_ref().map(|intent| intent.effect_id.clone()))
        .expect("process intent should remain durable");
    assert!(
        executing_store
            .load_tool_output(&effect_id)
            .expect("tool output lookup should work")
            .is_none()
    );
    assert!(
        executing_store
            .load_execution_receipts()
            .expect("Receipt lookup should work")
            .is_empty()
    );
    drop(executing_store);

    let connection = rusqlite::Connection::open(&database).expect("fault fixture should reopen");
    connection
        .execute_batch("DROP TRIGGER test_abort_process_effect_succeeded;")
        .expect("outcome fault trigger should remove");
    drop(connection);

    let recovered = bounded_output(xgen(&state_root).args(["resume", &run_id]))
        .expect("offline effect recovery should run");
    assert_eq!(recovered.status.code(), Some(30), "{}", stderr(&recovered));
    assert!(stderr(&recovered).contains("reason=effect_outcome_unknown"));
    assert_eq!(process_execution_count(&marker), 1);

    let unknown_store =
        SqliteRunStore::open_existing(&database).expect("unknown store should reopen");
    let unknown = unknown_store
        .load()
        .expect("unknown snapshot should load")
        .expect("unknown snapshot should exist");
    assert!(
        unknown
            .state
            .steps
            .values()
            .any(|step| step.status == StepStatus::EffectUnknown)
    );
    assert_eq!(
        unknown
            .records
            .iter()
            .filter(|record| matches!(record.event.body, RunEventBody::EffectBecameUnknown { .. }))
            .count(),
        1
    );
    drop(unknown_store);

    let repeated = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-executable",
        &executable_spec,
        "--allow-execute",
    ]))
    .expect("repeated execution approval should remain blocked");
    assert_eq!(repeated.status.code(), Some(30), "{}", stderr(&repeated));
    assert!(stderr(&repeated).contains("reason=effect_outcome_unknown"));
    assert_eq!(process_execution_count(&marker), 1);

    let after_repeat = SqliteRunStore::open_existing(&database)
        .expect("unknown store should reopen again")
        .load()
        .expect("repeated snapshot should load")
        .expect("repeated snapshot should exist");
    assert_eq!(after_repeat.state, unknown.state);
    assert_eq!(after_repeat.records, unknown.records);
}

#[test]
fn process_no_replay_child() {
    let Some(marker) = std::env::var_os("XGEN_PROCESS_NO_REPLAY_MARKER") else {
        return;
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
        .expect("execution marker should open");
    file.write_all(b"executed\n")
        .expect("execution marker should append");
    file.sync_all().expect("execution marker should be durable");
}

fn process_execution_count(marker: &Path) -> usize {
    fs::read_to_string(marker)
        .expect("execution marker should exist")
        .lines()
        .count()
}

#[test]
fn search_filters_generated_files_but_preserves_explicit_reads_and_offline_replay() {
    // Held-out Rust layout: globbed build files, a negated exception, and an exact ignored read.
    let fixture = tempdir().unwrap();
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("build")).unwrap();
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join(".ignore"), "/build/*\n!/build/keep.rs\n").unwrap();
    fs::write(workspace.join("build/keep.rs"), NEEDLE).unwrap();
    fs::write(
        workspace.join("build/generated.rs"),
        format!("{NEEDLE}\n").repeat(150),
    )
    .unwrap();
    fs::write(workspace.join("src/lib.rs"), NEEDLE).unwrap();
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "search",
            "Find source references",
            "xgeny.fs/search-text",
            &json!({"path": ".", "query": NEEDLE}),
        ),
        plan_response(
            "read",
            "Read an explicitly requested generated file",
            "xgeny.fs/read-text",
            &json!({"path": "build/generated.rs"}),
        ),
        completion_response(),
    ]);
    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-remote-model-egress",
        "Search source references, then read the generated file.",
    ]))
    .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), COMPLETION);
    let run_id = extract_run_id(&stderr(&output));
    let requests: Vec<_> = (0..3)
        .map(|_| server.requests.recv_timeout(TEST_TIMEOUT).unwrap())
        .collect();
    server.handle.join().unwrap();
    let after_search = planning_context(&requests[1]);
    let result = tool_output(&after_search, "xgeny.fs/search-text");
    let paths: Vec<_> = result["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["build/keep.rs", "src/lib.rs"]);
    assert_eq!(result["truncated"], false);
    let after_read = planning_context(&requests[2]);
    assert!(
        tool_output(&after_read, "xgeny.fs/read-text")["content"]
            .as_str()
            .unwrap()
            .contains(NEEDLE)
    );

    // Completed replay consumes the recorded observation, never current ignore rules.
    fs::write(workspace.join(".ignore"), "**\n").unwrap();
    let replay = bounded_output(xgen(&state_root).args(["resume", &run_id])).unwrap();
    assert_eq!(replay.status.code(), Some(0), "{}", stderr(&replay));
    assert_eq!(replay.stdout, output.stdout);
}

#[test]
fn dynamic_search_material_survives_process_pause_and_resume() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace should create");
    fs::write(
        workspace.join("notes.txt"),
        format!("prefix {NEEDLE} suffix"),
    )
    .expect("search fixture should write");
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "search_after_restart",
            "Search after an approval pause",
            "xgeny.fs/search-text",
            &json!({"path": ".", "query": NEEDLE}),
        ),
        completion_response(),
    ]);

    let first = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-remote-model-egress",
        "Search the workspace after an explicit approval pause.",
    ]))
    .expect("first process should pause");
    assert_eq!(first.status.code(), Some(10), "{}", stderr(&first));
    assert!(stderr(&first).contains("reason=read_approval_required"));
    let run_id = extract_run_id(&stderr(&first));
    let material_catalog = state_root
        .join("runs")
        .join(&run_id)
        .join("materials.sqlite3");
    assert!(material_catalog.is_file());
    assert_resume_scope_and_material_failures(&state_root, &workspace, &run_id, &material_catalog);

    let local = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-read",
    ]))
    .expect("local process should reconstruct and execute the search");
    assert_eq!(local.status.code(), Some(10), "{}", stderr(&local));
    assert!(stderr(&local).contains("reason=remote_model_egress_consent_required"));

    let completion = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-remote-model-egress",
    ]))
    .expect("remote continuation should complete");
    assert_eq!(completion.status.code(), Some(0), "{}", stderr(&completion));
    assert_eq!(String::from_utf8_lossy(&completion.stdout), COMPLETION);

    let _first_request = server
        .requests
        .recv_timeout(TEST_TIMEOUT)
        .expect("planning request should arrive");
    let second_request = server
        .requests
        .recv_timeout(TEST_TIMEOUT)
        .expect("completion request should arrive");
    server.handle.join().expect("provider server should finish");
    let context = planning_context(&second_request);
    assert!(
        tool_output(&context, "xgeny.fs/search-text")["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["path"] == "notes.txt")
    );
}

#[test]
fn atomic_write_requires_separate_approval_and_survives_process_resume() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "create_source",
            "Create one source file atomically",
            "xgeny.fs/write-atomic",
            &json!({
                "path": "src/generated.rs",
                "content": "pub const GENERATED: bool = true;\n",
                "expectedDigest": null
            }),
        ),
        completion_response(),
    ]);

    let first = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        "src",
        "--allow-remote-model-egress",
        "Create the requested source file.",
    ]))
    .expect("unapproved write should pause");
    assert_eq!(first.status.code(), Some(10), "{}", stderr(&first));
    assert!(stderr(&first).contains("reason=write_approval_required"));
    assert!(!workspace.join("src/generated.rs").exists());
    let run_id = extract_run_id(&stderr(&first));

    let local = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        "src",
        "--allow-write",
    ]))
    .expect("approved local write should execute without model access");
    assert_eq!(local.status.code(), Some(10), "{}", stderr(&local));
    assert!(stderr(&local).contains("reason=remote_model_egress_consent_required"));
    assert_eq!(
        fs::read_to_string(workspace.join("src/generated.rs")).unwrap(),
        "pub const GENERATED: bool = true;\n"
    );

    let completion = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--allow-dir",
        "src",
        "--allow-write",
        "--allow-remote-model-egress",
    ]))
    .expect("remote continuation should complete");
    assert_eq!(completion.status.code(), Some(0), "{}", stderr(&completion));
    assert_eq!(String::from_utf8_lossy(&completion.stdout), COMPLETION);

    let _first_request = server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
    let completion_request = server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
    server.handle.join().expect("provider server should finish");
    let context = planning_context(&completion_request);
    let output = tool_output(&context, "xgeny.fs/write-atomic");
    assert_eq!(output["path"], "workspace:primary/src/generated.rs");
    assert_eq!(output["changed"], true);
    assert_eq!(output["byteSize"], 34);
    assert!(output.get("content").is_none());

    let material_catalog = state_root
        .join("runs")
        .join(&run_id)
        .join("materials.sqlite3");
    let material_bytes = fs::read(material_catalog).unwrap();
    assert!(
        material_bytes
            .windows(b"GENERATED".len())
            .any(|bytes| bytes == b"GENERATED")
    );
}

#[test]
fn exact_patch_requires_write_approval_and_survives_process_resume() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    let source = "pub fn answer() -> u32 {\n    41\n}\n";
    fs::write(workspace.join("src/lib.rs"), source).expect("source should write");
    let expected_digest = test_sha256_digest(source.as_bytes());
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "patch_source",
            "Patch one exact source fragment",
            "xgeny.fs/apply-patch",
            &json!({
                "path": "src/lib.rs",
                "expectedDigest": expected_digest,
                "edits": [{
                    "oldText": "pub fn answer() -> u32 {\n    41\n}",
                    "newText": "pub fn answer() -> u32 {\n    42\n}"
                }]
            }),
        ),
        completion_response(),
    ]);

    let first = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        "src",
        "--allow-remote-model-egress",
        "Update the answer while preserving the rest of the file.",
    ]))
    .expect("unapproved patch should pause");
    assert_eq!(first.status.code(), Some(10), "{}", stderr(&first));
    assert!(stderr(&first).contains("reason=write_approval_required"));
    assert_eq!(
        fs::read_to_string(workspace.join("src/lib.rs")).unwrap(),
        source
    );
    let run_id = extract_run_id(&stderr(&first));

    let local = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        "src",
        "--allow-write",
    ]))
    .expect("approved local patch should execute without model access");
    assert_eq!(local.status.code(), Some(10), "{}", stderr(&local));
    assert!(stderr(&local).contains("reason=remote_model_egress_consent_required"));
    assert_eq!(
        fs::read_to_string(workspace.join("src/lib.rs")).unwrap(),
        "pub fn answer() -> u32 {\n    42\n}\n"
    );

    let completion = bounded_output(xgen(&state_root).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--allow-dir",
        "src",
        "--allow-write",
        "--allow-remote-model-egress",
    ]))
    .expect("remote continuation should complete");
    assert_eq!(completion.status.code(), Some(0), "{}", stderr(&completion));

    let _first_request = server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
    let completion_request = server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
    server.handle.join().expect("provider server should finish");
    let context = planning_context(&completion_request);
    let output = tool_output(&context, "xgeny.fs/apply-patch");
    assert_eq!(output["path"], "workspace:primary/src/lib.rs");
    assert_eq!(output["changed"], true);
    assert_eq!(output["editCount"], 1);
    assert!(output.get("content").is_none());
    assert!(output.get("edits").is_none());
}

#[test]
fn observations_of_a_file_changed_later_are_marked_for_the_planner() {
    // ADR-0069: a read or search covering a file that a later completed patch changed is marked
    // with that patch; unrelated and later observations are not. Every output stays.
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    let source = "pub fn answer() -> u32 {\n    41\n}\n";
    fs::write(workspace.join("src/lib.rs"), source).expect("source should write");
    fs::write(workspace.join("README.md"), "fixture workspace").expect("README should write");
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "read_source",
            "Read the source",
            "xgeny.fs/read-text",
            &json!({"path": "src/lib.rs"}),
        ),
        plan_response(
            "read_readme",
            "Read the README",
            "xgeny.fs/read-text",
            &json!({"path": "README.md"}),
        ),
        plan_response(
            "search_source",
            "Find the answer",
            "xgeny.fs/search-text",
            &json!({"path": "src", "query": "41"}),
        ),
        plan_response(
            "patch_source",
            "Patch the answer",
            "xgeny.fs/apply-patch",
            &json!({
                "path": "src/lib.rs",
                "expectedDigest": test_sha256_digest(source.as_bytes()),
                "edits": [{"oldText": "    41\n", "newText": "    42\n"}]
            }),
        ),
        plan_response(
            "reread_source",
            "Read the patched source",
            "xgeny.fs/read-text",
            &json!({"path": "src/lib.rs"}),
        ),
        completion_response(),
    ]);

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-write",
        "--allow-remote-model-egress",
        "Update the answer and confirm it.",
    ]))
    .expect("run should finish");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let requests: Vec<_> = (0..6)
        .map(|_| server.requests.recv_timeout(TEST_TIMEOUT).unwrap())
        .collect();
    server.handle.join().expect("provider server should finish");
    let before_patch = planning_context(&requests[3]);
    assert!(
        before_patch["toolOutputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|output| output.get("supersededBy").is_none())
    );
    let context = planning_context(&requests[5]);
    let steps = context["steps"].as_array().unwrap();
    let patch_step = steps[3]["stepId"].as_str().unwrap();
    let outputs = context["toolOutputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 5);
    for stale in [&outputs[0], &outputs[2]] {
        assert_eq!(stale["supersededBy"], patch_step, "{stale}");
        assert!(
            stale["supersededMeaning"]
                .as_str()
                .unwrap()
                .contains("may no longer match")
        );
    }
    assert_eq!(outputs[0]["output"]["content"], source);
    assert!(outputs[2]["output"]["matches"].as_array().is_some());
    assert_eq!(outputs[1]["output"]["content"], "fixture workspace");
    assert_eq!(outputs[3]["output"]["changed"], true);
    assert_eq!(
        outputs[4]["output"]["content"],
        "pub fn answer() -> u32 {\n    42\n}\n"
    );
    for fresh in [&outputs[1], &outputs[3], &outputs[4]] {
        assert!(fresh.get("supersededBy").is_none());
    }
}

#[test]
fn failed_read_is_shown_to_the_planner_and_the_task_can_complete() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace should create");
    // One byte over the read-text ceiling (ADR-0022) fails as a definite read-only failure.
    fs::write(workspace.join("large.txt"), "x".repeat(64 * 1024 + 1))
        .expect("large file should write");
    fs::write(workspace.join("README.md"), "fixture workspace").expect("README should write");
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "read_large",
            "Read the large file",
            "xgeny.fs/read-text",
            &json!({"path": "large.txt"}),
        ),
        plan_response(
            "read_readme",
            "Read the README instead",
            "xgeny.fs/read-text",
            &json!({"path": "README.md"}),
        ),
        completion_response(),
    ]);

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-remote-model-egress",
        "Summarize the workspace files.",
    ]))
    .expect("xgen run should finish");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), COMPLETION);

    let requests = (0..3)
        .map(|_| {
            server
                .requests
                .recv_timeout(TEST_TIMEOUT)
                .expect("every model request should arrive")
        })
        .collect::<Vec<_>>();
    server.handle.join().expect("provider server should finish");

    let after_failure = planning_context(&requests[1]);
    assert_eq!(after_failure["steps"][0]["status"], "failed");
    assert_eq!(after_failure["steps"][0]["failureCode"], "too_large");
    assert_eq!(after_failure["toolOutputs"], json!([]));
    let serialized = after_failure.to_string();
    assert!(!serialized.contains(path_text(&workspace)));
    let before_completion = planning_context(&requests[2]);
    assert_eq!(before_completion["steps"][1]["status"], "completed");
    assert!(before_completion["steps"][1].get("failureCode").is_none());
    assert_eq!(
        chronological_output_capabilities(&before_completion),
        ["xgeny.fs/read-text"]
    );

    let run_id = extract_run_id(&stderr(&output));
    let store = SqliteRunStore::open_existing_read_only(
        state_root.join("runs").join(&run_id).join("run.sqlite3"),
    )
    .expect("run store should open");
    let snapshot = store.load().unwrap().expect("run should exist");
    let mut statuses = snapshot
        .state
        .steps
        .values()
        .map(|step| step.status)
        .collect::<Vec<_>>();
    statuses.sort_by_key(|status| format!("{status:?}"));
    assert_eq!(statuses, [StepStatus::Completed, StepStatus::Failed]);
}

#[test]
fn a_file_larger_than_read_text_accepts_is_read_by_range_and_patched() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace should create");
    let mut large = format!("line {}\n", "x".repeat(32)).repeat(3_000);
    large.push_str("def target():\n    return 42\n");
    assert!(large.len() > 64 * 1024);
    fs::write(workspace.join("large.py"), &large).expect("large file should write");
    let server = SequentialServer::spawn_responses(vec![
        plan_response(
            "read_tail",
            "Read the definition at the end of the large file",
            "xgeny.fs/read-text-lines",
            &json!({"path": "large.py", "startLine": 3_001, "lineCount": 50}),
        ),
        // The whole-file digest from the range read is the patch precondition (ADR-0056).
        plan_response(
            "patch_tail",
            "Change the value the definition returns",
            "xgeny.fs/apply-patch",
            &json!({
                "path": "large.py",
                "expectedDigest": test_sha256_digest(large.as_bytes()),
                "edits": [{"oldText": "return 42", "newText": "return 43"}]
            }),
        ),
        completion_response(),
    ]);

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-write",
        "--allow-remote-model-egress",
        "Make target return 43.",
    ]))
    .expect("xgen run should finish");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        fs::read_to_string(workspace.join("large.py")).unwrap(),
        large.replace("return 42", "return 43")
    );

    let requests = (0..3)
        .map(|_| {
            server
                .requests
                .recv_timeout(TEST_TIMEOUT)
                .expect("every model request should arrive")
        })
        .collect::<Vec<_>>();
    server.handle.join().expect("provider server should finish");

    let first = planning_context(&requests[0]);
    let lines = capability(&first, "xgeny.fs/read-text-lines");
    assert_eq!(lines["effectClass"], "read_only");
    let after_read = planning_context(&requests[1]);
    let observed = tool_output(&after_read, "xgeny.fs/read-text-lines");
    assert_eq!(observed["content"], "def target():\n    return 42\n");
    assert_eq!(observed["startLine"], 3_001);
    assert_eq!(observed["endLine"], 3_002);
    assert_eq!(observed["totalLines"], 3_002);
    assert_eq!(observed["truncated"], false);
    assert_eq!(
        observed["fileDigest"],
        test_sha256_digest(large.as_bytes()).as_str()
    );
    let before_completion = planning_context(&requests[2]);
    assert_eq!(before_completion["steps"][1]["status"], "completed");
}

#[test]
fn rejected_write_is_shown_to_the_planner_and_a_corrected_write_completes() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    let source = "pub fn answer() -> u32 {\n    41\n}\n";
    fs::write(workspace.join("src/lib.rs"), source).expect("source should write");
    let patch = |key: &str, objective: &str, expected: &[u8]| {
        plan_response(
            key,
            objective,
            "xgeny.fs/apply-patch",
            &json!({
                "path": "src/lib.rs",
                "expectedDigest": test_sha256_digest(expected),
                "edits": [{
                    "oldText": "41",
                    "newText": "42"
                }]
            }),
        )
    };
    let server = SequentialServer::spawn_responses(vec![
        patch(
            "patch_stale",
            "Patch with a stale digest",
            b"stale contents",
        ),
        patch(
            "patch_current",
            "Patch the current contents",
            source.as_bytes(),
        ),
        completion_response(),
    ]);

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        "src",
        "--allow-write",
        "--allow-remote-model-egress",
        "Update the answer.",
    ]))
    .expect("xgen run should finish");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stderr(&output).contains("XGEN_STEP_FAILURE"));
    assert_eq!(
        fs::read_to_string(workspace.join("src/lib.rs")).unwrap(),
        source.replace("41", "42")
    );
    // Only the applied patch is reported, from the journal, not the rejected one (ADR-0057).
    let changes = stderr(&output)
        .lines()
        .filter(|line| line.starts_with("XGEN_CHANGE "))
        .map(|line| line.split_once(" kind=").unwrap().1.to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        changes,
        ["patch path=\"src/lib.rs\" changed=true added=1 removed=1"]
    );

    let requests = (0..3)
        .map(|_| {
            server
                .requests
                .recv_timeout(TEST_TIMEOUT)
                .expect("every model request should arrive")
        })
        .collect::<Vec<_>>();
    server.handle.join().expect("provider server should finish");
    // The rejected patch applied nothing, so the planner sees why and plans a corrected one
    // (ADR-0055).
    let after_rejection = planning_context(&requests[1]);
    assert_eq!(after_rejection["steps"][0]["status"], "failed");
    assert_eq!(
        after_rejection["steps"][0]["failureCode"],
        "precondition_conflict"
    );
    let meaning = after_rejection["steps"][0]["failureMeaning"]
        .as_str()
        .expect("a failed Step should state what its code means");
    assert!(meaning.contains("stale") && meaning.contains("Nothing was changed"));
    assert!(!after_rejection.to_string().contains(path_text(&workspace)));
    let before_completion = planning_context(&requests[2]);
    assert_eq!(before_completion["steps"][1]["status"], "completed");
}

#[test]
fn write_beyond_a_capability_limit_still_stops_the_run() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    // One byte over the apply-patch target ceiling recurs on every retry of this file.
    let source = format!("41\n{}", "x".repeat(8 * 1024 * 1024 - 2));
    fs::write(workspace.join("src/large.txt"), &source).expect("source should write");
    let server = SequentialServer::spawn_responses(vec![plan_response(
        "patch_large",
        "Patch the large file",
        "xgeny.fs/apply-patch",
        &json!({
            "path": "src/large.txt",
            "expectedDigest": test_sha256_digest(source.as_bytes()),
            "edits": [{
                "oldText": "41",
                "newText": "42"
            }]
        }),
    )]);

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        "src",
        "--allow-write",
        "--allow-remote-model-egress",
        "Update the answer.",
    ]))
    .expect("xgen run should finish");
    server.handle.join().expect("provider server should finish");
    assert!(
        stderr(&output).contains("reason=failed_work")
            && stderr(&output).contains("XGEN_STEP_FAILURE run_id=")
            && stderr(&output).contains(" code=too_large"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(workspace.join("src/large.txt")).unwrap(),
        source
    );
}

fn assert_resume_scope_and_material_failures(
    state_root: &Path,
    workspace: &Path,
    run_id: &str,
    material_catalog: &Path,
) {
    for mismatched_scope in [
        vec!["--allow-dir", "src"],
        vec!["--allow-file", "notes.txt"],
    ] {
        let mut command = xgen(state_root);
        command.args(["resume", run_id, "--workspace", path_text(workspace)]);
        command.args(mismatched_scope);
        command.arg("--allow-read");
        let rejected = bounded_output(&mut command).expect("mismatched scope should be rejected");
        assert_eq!(rejected.status.code(), Some(64), "{}", stderr(&rejected));
    }

    let held_catalog = material_catalog.with_extension("sqlite3.held");
    fs::rename(material_catalog, &held_catalog).expect("material catalog should move aside");
    let missing_catalog = bounded_output(xgen(state_root).args([
        "resume",
        run_id,
        "--workspace",
        path_text(workspace),
        "--allow-dir",
        ".",
        "--allow-read",
    ]))
    .expect("missing material catalog should fail closed");
    assert_eq!(
        missing_catalog.status.code(),
        Some(70),
        "{}",
        stderr(&missing_catalog)
    );
    fs::rename(&held_catalog, material_catalog).expect("material catalog should restore");
}

#[test]
#[allow(clippy::too_many_lines)]
fn public_cli_discovers_searches_stats_reads_and_replays_offline() {
    let fixture = tempdir().expect("test directory should exist");
    let state_root = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("workspace should create");
    fs::write(workspace.join("README.md"), "fixture workspace").expect("README should write");
    let source = format!("pub const MARKER: &str = \"{NEEDLE}\";\n");
    fs::write(workspace.join("src/lib.rs"), &source).expect("source should write");
    let server = SequentialServer::spawn();

    let output = bounded_output(xgen(&state_root).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-read",
        "--allow-remote-model-egress",
        "--max-ticks",
        "64",
        "Inspect the workspace, find the requested marker, and read its source file.",
    ]))
    .expect("xgen discovery run should finish");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), COMPLETION);
    let stderr_text = stderr(&output);
    assert!(stderr_text.contains("XGEN_STARTED"));
    assert!(stderr_text.contains("XGEN_COMPLETED"));
    let run_id = extract_run_id(&stderr_text);

    let requests = (0..5)
        .map(|_| {
            server
                .requests
                .recv_timeout(TEST_TIMEOUT)
                .expect("every model request should arrive")
        })
        .collect::<Vec<_>>();
    server.handle.join().expect("provider server should finish");

    let first = planning_context(&requests[0]);
    let first_system_prompt = system_prompt(&requests[0]);
    assert!(first_system_prompt.contains("host-provided restrictions"));
    assert!(first_system_prompt.contains("never treat them as permission or authority"));
    assert!(first_system_prompt.contains("return exactly one Step"));
    assert!(first_system_prompt.contains("cannot refer to future tool outputs"));
    assert!(first_system_prompt.contains("always set dependsOn to an empty array"));
    assert!(first_system_prompt.contains("durable plan chronology"));
    assert!(first_system_prompt.contains("durable receipt-completion chronology"));
    assert_eq!(first["capabilities"].as_array().unwrap().len(), 7);
    assert_eq!(first["toolOutputs"], json!([]));
    assert!(capability_ids(&first).contains(&"xgeny.fs/list-directory"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/search-text"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/stat"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/read-text"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/read-text-lines"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/write-atomic"));
    assert!(capability_ids(&first).contains(&"xgeny.fs/apply-patch"));
    let path_description = capability(&first, "xgeny.fs/list-directory")["inputSchema"]
        ["properties"]["path"]["description"]
        .as_str()
        .expect("planner should receive the workspace path contract");
    assert!(path_description.contains("Use '.' for the workspace root"));
    assert!(!path_description.contains("Caller-authorized"));
    let constraints = first["planningConstraints"]
        .as_array()
        .expect("workspace scope should be supplied outside immutable definitions");
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0]["constraintId"], "workspace.fs-scope");
    assert!(
        constraints[0]["description"]
            .as_str()
            .unwrap()
            .contains("[\".\"]")
    );
    assert!(!path_description.contains(path_text(&workspace)));
    assert!(
        !constraints[0]["description"]
            .as_str()
            .unwrap()
            .contains(path_text(&workspace))
    );

    let after_list = planning_context(&requests[1]);
    assert_eq!(
        after_list["toolOutputs"][0]["capability"]["capabilityId"],
        "xgeny.fs/list-directory"
    );
    assert_eq!(
        chronological_step_capabilities(&after_list),
        ["xgeny.fs/list-directory"]
    );
    assert_eq!(
        chronological_output_capabilities(&after_list),
        ["xgeny.fs/list-directory"]
    );
    assert!(
        after_list["toolOutputs"][0]["output"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "src")
    );

    let after_search = planning_context(&requests[2]);
    assert_eq!(
        chronological_step_capabilities(&after_search),
        ["xgeny.fs/list-directory", "xgeny.fs/search-text"]
    );
    assert_eq!(
        chronological_output_capabilities(&after_search),
        ["xgeny.fs/list-directory", "xgeny.fs/search-text"]
    );
    assert!(
        tool_output(&after_search, "xgeny.fs/search-text")["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["path"] == "src/lib.rs")
    );

    let after_stat = planning_context(&requests[3]);
    assert_eq!(
        chronological_step_capabilities(&after_stat),
        [
            "xgeny.fs/list-directory",
            "xgeny.fs/search-text",
            "xgeny.fs/stat",
        ]
    );
    assert_eq!(
        chronological_output_capabilities(&after_stat),
        [
            "xgeny.fs/list-directory",
            "xgeny.fs/search-text",
            "xgeny.fs/stat",
        ]
    );
    assert_eq!(tool_output(&after_stat, "xgeny.fs/stat")["kind"], "file");
    assert_eq!(
        tool_output(&after_stat, "xgeny.fs/stat")["sizeBytes"],
        u64::try_from(source.len()).unwrap()
    );

    let after_read = planning_context(&requests[4]);
    assert_eq!(
        chronological_step_capabilities(&after_read),
        [
            "xgeny.fs/list-directory",
            "xgeny.fs/search-text",
            "xgeny.fs/stat",
            "xgeny.fs/read-text",
        ]
    );
    assert_eq!(
        chronological_output_capabilities(&after_read),
        [
            "xgeny.fs/list-directory",
            "xgeny.fs/search-text",
            "xgeny.fs/stat",
            "xgeny.fs/read-text",
        ]
    );
    assert_eq!(
        tool_output(&after_read, "xgeny.fs/read-text")["content"],
        source
    );

    let run_directory = state_root.join("runs").join(&run_id);
    let database = run_directory.join("run.sqlite3");
    let store = SqliteRunStore::open_existing(&database).expect("Run store should reopen");
    assert_eq!(
        store
            .load_execution_receipts()
            .expect("receipts should load")
            .len(),
        4
    );
    drop(store);
    let material_catalog = run_directory.join("materials.sqlite3");
    assert!(material_catalog.is_file());
    let manifest = fs::read(run_directory.join("manifest.json")).expect("manifest should read");
    for forbidden in [NEEDLE, "src/lib.rs", path_text(&workspace)] {
        assert!(!String::from_utf8_lossy(&manifest).contains(forbidden));
    }

    fs::remove_dir_all(&workspace).expect("completed workspace should be removable");
    fs::remove_file(&material_catalog).expect("completed material catalog should be removable");
    let replay = bounded_output(xgen(&state_root).args(["resume", &run_id]))
        .expect("offline completion should replay");
    assert_eq!(replay.status.code(), Some(0), "{}", stderr(&replay));
    assert_eq!(String::from_utf8_lossy(&replay.stdout), COMPLETION);
}

fn capability_ids(context: &Value) -> Vec<&str> {
    context["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|capability| capability["capability"]["capabilityId"].as_str().unwrap())
        .collect()
}

fn capability<'a>(context: &'a Value, capability_id: &str) -> &'a Value {
    context["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["capability"]["capabilityId"] == capability_id)
        .expect("requested capability should exist")
}

fn tool_output<'a>(context: &'a Value, capability_id: &str) -> &'a Value {
    &context["toolOutputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|output| output["capability"]["capabilityId"] == capability_id)
        .expect("requested capability output should exist")["output"]
}

fn chronological_step_capabilities(context: &Value) -> Vec<&str> {
    context["steps"]
        .as_array()
        .expect("steps should be an array")
        .iter()
        .map(|step| {
            step["capability"]["capabilityId"]
                .as_str()
                .expect("Step capability ID should be text")
        })
        .collect()
}

fn chronological_output_capabilities(context: &Value) -> Vec<&str> {
    context["toolOutputs"]
        .as_array()
        .expect("ToolOutputs should be an array")
        .iter()
        .map(|output| {
            output["capability"]["capabilityId"]
                .as_str()
                .expect("ToolOutput capability ID should be text")
        })
        .collect()
}

fn plan_response(key: &str, objective: &str, capability_id: &str, arguments: &Value) -> Vec<u8> {
    provider_response(&json!({
        "formatVersion": 1,
        "kind": "plan",
        "steps": [{
            "key": key,
            "objective": objective,
            "dependsOn": [],
            "capability": {
                "capabilityId": capability_id,
                "contractVersion": match capability_id {
                    "xgen.tools/describe" | "xgeny.fs/read-text-lines" | "xgeny.fs/apply-patch" | "xgeny.fs/search-text" => "2.0.0",
                    _ => "1.0.0",
                }
            },
            "arguments": arguments
        }],
        "summary": ""
    }))
}

fn completion_response() -> Vec<u8> {
    provider_response(&json!({
        "formatVersion": 1,
        "kind": "completion_candidate",
        "steps": [],
        "summary": COMPLETION
    }))
}

fn provider_response(content: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": "RAW-DISCOVERY-RESPONSE",
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content.to_string()},
            "finish_reason": "stop"
        }]
    }))
    .expect("provider response should serialize")
}

fn xgen(state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xgen"));
    command
        .env("XGEN_STATE_HOME", state_root)
        .env_remove("XGEN_OPENAI_API_KEY");
    command
}

fn bounded_output(command: &mut Command) -> io::Result<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "xgen child did not exit before the test deadline",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout)?;
    child.stderr.take().unwrap().read_to_end(&mut stderr)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).expect("request should read");
        assert_ne!(read, 0, "request ended before headers completed");
        request.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = find_header_end(&request) {
            let header = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .expect("Content-Length should exist");
            let target = header_end + 4 + content_length;
            while request.len() < target {
                let read = stream.read(&mut chunk).expect("request body should read");
                assert_ne!(read, 0, "request ended before body completed");
                request.extend_from_slice(&chunk[..read]);
            }
            request.truncate(target);
            return request;
        }
    }
}

fn planning_context(request: &[u8]) -> Value {
    let header_end = find_header_end(request).expect("HTTP headers should end");
    let body: Value = serde_json::from_slice(&request[header_end + 4..]).unwrap();
    let prompt: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap())
        .expect("planner prompt should be JSON");
    prompt["planningContext"].clone()
}

fn system_prompt(request: &[u8]) -> String {
    let header_end = find_header_end(request).expect("HTTP headers should end");
    let body: Value = serde_json::from_slice(&request[header_end + 4..]).unwrap();
    body["messages"][0]["content"]
        .as_str()
        .expect("system prompt should be text")
        .to_owned()
}

fn accept_with_timeout(listener: &TcpListener) -> io::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    // A sequential fixture can wait for an approved process invocation between model turns.
    // Match the outer child-process deadline so a loaded CI runner cannot close the mock provider
    // while the bounded CLI process is still legitimately running.
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(TEST_TIMEOUT))?;
                stream.set_write_timeout(Some(TEST_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "planner call did not connect before the deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn extract_run_id(stderr: &str) -> String {
    stderr
        .split_whitespace()
        .find_map(|field| field.strip_prefix("run_id="))
        .expect("run ID should be announced")
        .to_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("test paths should be UTF-8")
}

fn test_sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{encoded}")
}

#[cfg(unix)]
fn discovery_runtime(state: &Path, runtime: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::create_dir_all(runtime).unwrap();
    let collections = state.join("tool-collections");
    fs::create_dir_all(&collections).unwrap();
    fs::set_permissions(state, fs::Permissions::from_mode(0o700)).unwrap();
    for (name, tool, parameter) in [
        ("assets", "readAsset", "asset_id"),
        ("calendar", "readEvent", "eventId"),
    ] {
        fs::write(
            collections.join(format!("{name}.json.gz")),
            json!({"tool":tool,"parameter":parameter,"digest":"a".repeat(64)}).to_string(),
        )
        .unwrap();
    }
    let worker = runtime.join("uv");
    fs::write(&worker, r"#!/usr/bin/python3
import sys,json
from pathlib import Path
r=json.load(sys.stdin)
root=Path(r['root'])
def summary(name,row):
 return dict(collection=name,artifact_digest=row['digest'],backend_version=row.get('backend_version','0.47.0'),execution_enabled=False,tool_count=1,source_count=1)
if r['operation']=='list':
 out=dict(ok=True,collections=[dict(summary(p.name[:-8],json.loads(p.read_text())),status=json.loads(p.read_text()).get('status','ready')) for p in sorted(root.glob('*.json.gz'))])
elif r['operation']=='language_hints':
 assert '--offline' in sys.argv
 rows={n:json.loads((root/(n+'.json.gz')).read_text()) for n in r['snapshots']}
 out=dict(ok=False,error='collection_snapshot_changed') if any(rows[n]['digest']!=d for n,d in r['snapshots'].items()) else dict(ok=True,collections={n:dict(documentation_script='Latin',script_differs=False) for n in rows})
else:
 assert '--offline' in sys.argv
 row=json.loads((root/(r['name']+'.json.gz')).read_text())
 with (root/'invocations').open('a') as log: log.write(r['operation']+'\n')
 if r['expected_digest']!=row['digest']: out=dict(ok=False,error='collection_snapshot_changed')
 else:
  out=dict(ok=True,**summary(r['name'],row))
  if r['operation']=='search': out.update(candidates=[dict(tool=row['tool'],description='Read one record',score=1.0)],possible_producers=[],omitted_producers=0,relations_verified_by_execution=False)
  else:
   assert r['tool']==row['tool']
   out.update(tool=dict(name=row['tool'],parameters=[dict(index=0,name=dict(text=row['parameter'],omitted=False),schema=dict(type='string'),schema_digest='b'*64,complete=True,required=True)],metadata=dict(api_contract=dict(method='get',path='/records/{id}'))),contract_kind='discovery_view',effect_class='unclassified',view_version=2,complete=False,full_tool_digest='c'*64,parameter_page=dict(offset=0,total=1,next_offset=None),http_read=dict(supported=False,error='http_original_source_unavailable'))
print(json.dumps(out))
").unwrap();
    fs::set_permissions(worker, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
fn tool_discovery_is_approved_receipted_and_frozen_across_resume() {
    for (collection, target, parameter) in [
        ("assets", "readAsset", "asset_id"),
        ("calendar", "readEvent", "eventId"),
    ] {
        let fixture = tempdir().unwrap();
        let state = fixture.path().join("state");
        let workspace = fixture.path().join("workspace");
        let runtime = fixture.path().join("runtime");
        fs::create_dir(&workspace).unwrap();
        discovery_runtime(&state, &runtime);
        let server = SequentialServer::spawn_responses(vec![
            plan_response(
                "search",
                "Find candidate tools",
                "xgen.tools/search",
                &json!({"collection":collection,"query":"read record detail","topK":1}),
            ),
            plan_response(
                "describe",
                "Inspect exact contract",
                "xgen.tools/describe",
                &json!({"collection":collection,"tool":target}),
            ),
            completion_response(),
        ]);
        let first = bounded_output(xgen(&state).env("PATH", &runtime).args([
            "run",
            "--workspace",
            path_text(&workspace),
            "--base-url",
            &server.base_url,
            "--model",
            MODEL,
            "--tokenizer",
            TOKENIZER,
            "--allow-dir",
            ".",
            "--allow-remote-model-egress",
            "Find a tool and explain its required input.",
        ]))
        .unwrap();
        assert_eq!(first.status.code(), Some(10), "{}", stderr(&first));
        assert!(stderr(&first).contains("read_approval_required"));
        let run_id = extract_run_id(&stderr(&first));
        let database = state.join("runs").join(&run_id).join("run.sqlite3");
        assert!(
            SqliteRunStore::open_existing(&database)
                .unwrap()
                .load_execution_receipts()
                .unwrap()
                .is_empty()
        );
        assert!(!state.join("tool-collections/invocations").exists());
        // A later collection is not silently added to an existing Run's scope.
        fs::write(
            state.join("tool-collections/later.json.gz"),
            "not a valid catalog",
        )
        .unwrap();
        let local = bounded_output(xgen(&state).env("PATH", &runtime).args([
            "resume",
            &run_id,
            "--workspace",
            path_text(&workspace),
            "--allow-dir",
            ".",
            "--allow-read",
        ]))
        .unwrap();
        assert_eq!(local.status.code(), Some(10), "{}", stderr(&local));
        let result = bounded_output(xgen(&state).env("PATH", &runtime).args([
            "resume",
            &run_id,
            "--workspace",
            path_text(&workspace),
            "--base-url",
            &server.base_url,
            "--allow-dir",
            ".",
            "--allow-read",
            "--allow-remote-model-egress",
        ]))
        .unwrap();
        assert!(result.status.success(), "{}", stderr(&result));
        let requests: Vec<_> = (0..3)
            .map(|_| server.requests.recv_timeout(TEST_TIMEOUT).unwrap())
            .collect();
        server.handle.join().unwrap();
        let initial = planning_context(&requests[0]);
        assert!(capability_ids(&initial).contains(&"xgen.tools/search"));
        assert!(capability_ids(&initial).contains(&"xgen.tools/describe"));
        let final_context = planning_context(&requests[2]);
        let described = tool_output(&final_context, "xgen.tools/describe");
        assert_eq!(described["tool"]["name"], target);
        assert_eq!(
            described["tool"]["parameters"][0]["name"]["text"],
            parameter
        );
        assert_eq!(described["execution_enabled"], false);
        assert_eq!(described["artifact_digest"], "a".repeat(64));
        assert!(!capability_ids(&final_context).contains(&target));
        assert!(
            !final_context["planningConstraints"]
                .to_string()
                .contains("later")
        );
        let store = SqliteRunStore::open_existing(&database).unwrap();
        assert_eq!(store.load_execution_receipts().unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(state.join("tool-collections/invocations")).unwrap(),
            "search\ndescribe_view\n"
        );
        let manifest: Value = serde_json::from_slice(
            &fs::read(state.join("runs").join(run_id).join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["record"]["toolDiscoverySnapshots"][collection],
            "a".repeat(64)
        );
    }
}

#[test]
#[cfg(unix)]
fn tool_discovery_rejects_unknown_collections_and_candidate_execution() {
    for (capability_id, input, code) in [
        (
            "xgen.tools/search",
            json!({"collection":"unknown","query":"record","topK":1}),
            "invocation_invalid",
        ),
        (
            "readAsset",
            json!({"asset_id":"one"}),
            "capability_unavailable",
        ),
    ] {
        let fixture = tempdir().unwrap();
        let state = fixture.path().join("state");
        let workspace = fixture.path().join("workspace");
        let runtime = fixture.path().join("runtime");
        fs::create_dir(&workspace).unwrap();
        discovery_runtime(&state, &runtime);
        let server = SequentialServer::spawn_responses(
            (0..=xgen_cli::MAX_REJECTED_RESPONSE_RETRIES)
                .map(|_| {
                    plan_response(
                        "invalid",
                        "Attempt unavailable operation",
                        capability_id,
                        &input,
                    )
                })
                .collect(),
        );
        let result = bounded_output(xgen(&state).env("PATH", runtime).args([
            "run",
            "--workspace",
            path_text(&workspace),
            "--base-url",
            &server.base_url,
            "--model",
            MODEL,
            "--tokenizer",
            TOKENIZER,
            "--allow-dir",
            ".",
            "--allow-read",
            "--allow-remote-model-egress",
            "Inspect the tool.",
        ]))
        .unwrap();
        assert_eq!(result.status.code(), Some(20), "{}", stderr(&result));
        let run_id = extract_run_id(&stderr(&result));
        let store =
            SqliteRunStore::open_existing(state.join("runs").join(run_id).join("run.sqlite3"))
                .unwrap();
        assert!(store.load_execution_receipts().unwrap().is_empty());
        assert!(!state.join("tool-collections/invocations").exists());
        let requests: Vec<_> = (0..=xgen_cli::MAX_REJECTED_RESPONSE_RETRIES)
            .map(|_| server.requests.recv_timeout(TEST_TIMEOUT).unwrap())
            .collect();
        server.handle.join().unwrap();
        // ADR-0064: each retry carries the Core class of the rejection before it.
        let rejection = &planning_context(&requests[1])["previousResponseRejection"];
        assert_eq!(rejection["reason"], "proposal_rejected", "{rejection}");
        assert_eq!(rejection["proposalRejection"], code, "{rejection}");
    }
}

#[test]
fn a_truncated_response_is_sampled_again_with_its_reason() {
    // ADR-0073: an output that hit the token limit is retried like an invalid one.
    let fixture = tempdir().unwrap();
    let state = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let answer = provider_response(&json!({
        "formatVersion": 1, "kind": "response_candidate", "steps": [], "summary": "answer"
    }));
    let mut truncated: Value = serde_json::from_slice(&answer).unwrap();
    truncated["choices"][0]["finish_reason"] = json!("length");
    let server =
        SequentialServer::spawn_responses(vec![serde_json::to_vec(&truncated).unwrap(), answer]);
    let result = bounded_output(xgen(&state).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-remote-model-egress",
        "--conversation-responses",
        "Answer.",
    ]))
    .unwrap();
    let requests: Vec<_> = (0..2)
        .map(|_| server.requests.recv_timeout(TEST_TIMEOUT).unwrap())
        .collect();
    server.handle.join().unwrap();
    assert!(
        planning_context(&requests[0])
            .get("previousResponseRejection")
            .is_none()
    );
    let rejection = &planning_context(&requests[1])["previousResponseRejection"];
    assert_eq!(rejection["reason"], "output_truncated", "{rejection}");
    assert!(
        rejection["meaning"]
            .to_string()
            .contains("shorter proposal")
    );
    assert_eq!(result.status.code(), Some(0), "{}", stderr(&result));
}

#[test]
#[cfg(unix)]
fn run_names_collections_that_must_be_rebuilt_and_does_not_start() {
    let fixture = tempdir().unwrap();
    let state = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    let runtime = fixture.path().join("runtime");
    fs::create_dir(&workspace).unwrap();
    discovery_runtime(&state, &runtime);
    fs::write(
        state.join("tool-collections/assets.json.gz"),
        json!({"tool":"readAsset","parameter":"asset_id","digest":"a".repeat(64),
            "status":"rebuild_required","backend_version":"0.46.0"})
        .to_string(),
    )
    .unwrap();
    let server = SequentialServer::spawn_responses(vec![completion_response()]);
    let result = bounded_output(xgen(&state).env("PATH", runtime).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-remote-model-egress",
        "Inspect the tool.",
    ]))
    .unwrap();
    assert!(!result.status.success(), "{}", stderr(&result));
    let errors = stderr(&result);
    assert!(errors.contains("XGEN_TOOLS error=collection_rebuild_required collection=assets"));
    assert!(!errors.contains("collection=calendar"));
    assert!(
        server
            .requests
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    assert!(!state.join("tool-collections/invocations").exists());
}

#[test]
#[cfg(unix)]
fn tool_discovery_changed_snapshot_fails_before_verified_output() {
    let fixture = tempdir().unwrap();
    let state = fixture.path().join("state");
    let workspace = fixture.path().join("workspace");
    let runtime = fixture.path().join("runtime");
    fs::create_dir(&workspace).unwrap();
    discovery_runtime(&state, &runtime);
    let server = SequentialServer::spawn_responses(vec![plan_response(
        "search",
        "Find record",
        "xgen.tools/search",
        &json!({"collection":"assets","query":"detail","topK":1}),
    )]);
    let first = bounded_output(xgen(&state).env("PATH", &runtime).args([
        "run",
        "--workspace",
        path_text(&workspace),
        "--base-url",
        &server.base_url,
        "--model",
        MODEL,
        "--tokenizer",
        TOKENIZER,
        "--allow-dir",
        ".",
        "--allow-remote-model-egress",
        "Inspect the tool.",
    ]))
    .unwrap();
    assert_eq!(first.status.code(), Some(10));
    let run_id = extract_run_id(&stderr(&first));
    fs::write(
        state.join("tool-collections/assets.json.gz"),
        json!({"tool":"readAsset","parameter":"asset_id","digest":"b".repeat(64)}).to_string(),
    )
    .unwrap();
    let resumed = bounded_output(xgen(&state).env("PATH", runtime).args([
        "resume",
        &run_id,
        "--workspace",
        path_text(&workspace),
        "--allow-dir",
        ".",
        "--allow-read",
    ]))
    .unwrap();
    assert!(!resumed.status.success(), "{}", stderr(&resumed));
    let store =
        SqliteRunStore::open_existing(state.join("runs").join(run_id).join("run.sqlite3")).unwrap();
    let state = store.load_current().unwrap().unwrap();
    for step in state.steps.values() {
        if let Some(intent) = &step.intent {
            assert_eq!(
                store
                    .load_tool_output(&intent.effect_id)
                    .unwrap()
                    .unwrap()
                    .output()["ok"],
                false
            );
        }
    }
    let receipts = store.load_execution_receipts().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        serde_json::to_value(&receipts[0]).unwrap()["status"],
        "failed"
    );
    server.requests.recv_timeout(TEST_TIMEOUT).unwrap();
    server.handle.join().unwrap();
}
