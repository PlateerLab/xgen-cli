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
/// Summary-enrichment answers carry generated text for every operation; the worker allows 4 MiB.
const MAX_REQUEST: usize = 4 * 1024 * 1024;
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
    /// Connect an imported collection to a host-approved read API base URL.
    Connect {
        #[arg(long)]
        name: String,
        #[arg(long)]
        base_url: String,
        /// Confirm that this system's GET operations are non-mutating read operations.
        #[arg(long)]
        allow_get: bool,
        /// Declare an exact `OpenAPI` operationId to be a non-mutating read.
        #[arg(long)]
        read_operation: Vec<String>,
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
    /// List saved collections and whether each one can be used.
    List,
    /// Rebuild collections saved by another backend version from the sources kept at import.
    /// Runs pinned to the previous snapshot fail closed when they resume.
    Rebuild {
        #[arg(long, required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,
        /// Rebuild every saved collection that needs it.
        #[arg(long)]
        all: bool,
    },
    /// Remove an HTTP connection and its stored credential; the collection stays searchable.
    Disconnect {
        #[arg(long)]
        name: String,
    },
    /// Remove a collection together with its HTTP connection and stored credential.
    Remove {
        #[arg(long)]
        name: String,
    },
    /// Add generated operation summaries in other languages to a collection's search index.
    /// Operation summaries, descriptions, tags and parameter names are sent to the model
    /// profile; the planner keeps seeing the source summaries. Local JSON sources only.
    Enrich {
        #[arg(long)]
        name: String,
        /// Languages to add, comma separated: ko, en, ja, zh.
        #[arg(long, value_delimiter = ',', required = true)]
        languages: Vec<String>,
        /// Saved model profile to use instead of the active one.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Retrieve candidates and possible prerequisite producers; never executes them.
    Search {
        #[arg(long)]
        name: String,
        query: String,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=20))]
        top_k: u8,
    },
    /// Inspect HTTP contract support for a saved collection without API or credential access.
    CheckHttp {
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 0)]
        offset: u32,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=500))]
        limit: u16,
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
            read_operation,
            bearer_env,
            bearer,
            token_stdin,
        } => {
            if allow_get || !read_operation.is_empty() {
                crate::http_read::connect(
                    &name,
                    base_url,
                    (allow_get, read_operation),
                    bearer_env,
                    token_stdin,
                    bearer,
                )
            } else {
                Err("http_get_consent_required")
            }
        }
        ToolsCommand::Disconnect { name } => {
            crate::http_read::disconnect(&name).and_then(|removed| {
                let mut result = json!({"ok": true, "collection": name});
                let fields = removed.ok_or("http_connection_not_found")?;
                result.as_object_mut().expect("object").extend(fields);
                Ok(result)
            })
        }
        ToolsCommand::Remove { name } => remove(&name),
        ToolsCommand::Import { name, source } => import(&name, source),
        ToolsCommand::Rebuild { name, all } => rebuild(name, all),
        // The host binds a model profile first; see `enrich`.
        ToolsCommand::Enrich { .. } => Err("summary_enrichment_requires_model"),
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
        ToolsCommand::Connect { .. } | ToolsCommand::Disconnect { .. } => {
            return Err("unsupported_transport_operation");
        }
        ToolsCommand::List => json!({"operation": "list"}),
        ToolsCommand::Rebuild { name, .. } => {
            json!({"operation": "rebuild", "name": name.ok_or("invalid_collection_name")?})
        }
        ToolsCommand::Remove { name } => json!({"operation": "remove", "name": name}),
        ToolsCommand::Enrich {
            name, languages, ..
        } => {
            json!({"operation": "enrichment_plan", "name": name, "languages": languages, "completed": {}})
        }
        ToolsCommand::Search { name, query, top_k } => {
            if query.trim().is_empty() || query.len() > 4096 {
                return Err("invalid_query");
            }
            json!({"operation": "search", "name": name, "query": query, "top_k": top_k})
        }
        ToolsCommand::CheckHttp {
            name,
            offset,
            limit,
        } => {
            json!({"operation":"check_http", "name":name, "offset":offset, "limit":limit})
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
    if matches!(
        value["operation"].as_str(),
        Some("describe" | "describe_view" | "check_http")
    ) {
        let snapshots = crate::graph_discovery::DiscoveryCatalog::discover()
            .map_err(|()| "http_catalog_unavailable")?;
        let connections =
            crate::http_read::discover(&snapshots).map_err(|()| "http_connection_unavailable")?;
        if let Some(connection) = value["name"]
            .as_str()
            .and_then(|name| connections.get(name))
        {
            value["http_policy"] = connection.policy();
        }
    }
    Ok(value)
}

/// One summary-enrichment model answer: the JSON value and the provider-reported token usage.
pub type EnrichmentAnswer = (Value, Option<u64>, Option<u64>);

/// Rounds of model requests: the first pass and one retry of operations whose answer was
/// missing or invalid, as measured (ADR-0091).
const ENRICHMENT_ROUNDS: usize = 2;

/// Add generated summaries to a collection with a host-bound JSON task.
///
/// The worker plans the requests and validates every answer (language script, length); this
/// function only carries prompts to `complete` and answers back, then stores the result once.
#[must_use]
pub fn enrich<F>(name: &str, languages: Vec<String>, model: &str, complete: F) -> ExitCode
where
    F: FnMut(&str, &str, &Value) -> Result<EnrichmentAnswer, &'static str>,
{
    if ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed)).is_err() {
        eprintln!(
            "{}",
            json!({"ok": false, "error": "interrupt_handler_unavailable"})
        );
        return ExitCode::FAILURE;
    }
    let result = enrich_collection(name, languages, model, complete);
    match result {
        Ok(response) => {
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

fn enrich_collection<F>(
    name: &str,
    languages: Vec<String>,
    model: &str,
    mut complete: F,
) -> Result<Value, &'static str>
where
    F: FnMut(&str, &str, &Value) -> Result<EnrichmentAnswer, &'static str>,
{
    let base = request(ToolsCommand::Enrich {
        name: name.to_owned(),
        languages,
        profile: None,
    })?;
    let mut completed = json!({});
    let mut calls = 0_u64;
    let mut failures = serde_json::Map::new();
    let (mut prompt_tokens, mut completion_tokens) = (Some(0_u64), Some(0_u64));
    let mut expected_digest = None;
    for _ in 0..ENRICHMENT_ROUNDS {
        let mut plan = base.clone();
        plan["completed"] = completed.clone();
        let planned = invoke(&plan)?;
        if planned.get("ok") != Some(&Value::Bool(true)) {
            return Ok(planned);
        }
        let digest = planned.get("artifact_digest").cloned();
        if expected_digest.get_or_insert_with(|| digest.clone()) != &digest {
            return Err("collection_snapshot_changed");
        }
        let batches = planned["batches"]
            .as_array()
            .ok_or("invalid_worker_response")?;
        if batches.is_empty() {
            break;
        }
        let mut answers = Vec::with_capacity(batches.len());
        for batch in batches {
            if INTERRUPTED.load(Ordering::Relaxed) {
                return Err("interrupted");
            }
            let (Some(system), Some(user)) = (batch["system"].as_str(), batch["user"].as_str())
            else {
                return Err("invalid_worker_response");
            };
            calls += 1;
            match complete(system, user, &batch["schema"]) {
                Ok((value, prompt, completion)) => {
                    prompt_tokens = prompt_tokens.zip(prompt).map(|(sum, value)| sum + value);
                    completion_tokens = completion_tokens
                        .zip(completion)
                        .map(|(sum, value)| sum + value);
                    answers.push(json!({"keys": batch["keys"], "languages": batch["languages"], "value": value}));
                }
                Err(code) => {
                    let count = failures.get(code).and_then(Value::as_u64).unwrap_or(0);
                    failures.insert(code.to_owned(), json!(count + 1));
                }
            }
        }
        let mut collect = base.clone();
        collect["operation"] = json!("enrichment_apply");
        collect["completed"] = completed;
        collect["answers"] = json!(answers);
        collect["expected_digest"] = expected_digest.clone().flatten().unwrap_or(Value::Null);
        let collected = invoke(&collect)?;
        if collected.get("ok") != Some(&Value::Bool(true)) {
            return Ok(collected);
        }
        completed = collected["completed"].clone();
    }
    if completed.as_object().is_none_or(serde_json::Map::is_empty) {
        return Ok(json!({"ok": false, "error": "summary_enrichment_failed",
                         "model_calls": calls, "model_failures": failures}));
    }
    let mut store = base;
    store["operation"] = json!("enrichment_apply");
    store["completed"] = completed;
    store["answers"] = json!([]);
    store["store"] = json!(true);
    store["model"] = json!(model);
    store["expected_digest"] = expected_digest.flatten().unwrap_or(Value::Null);
    let mut stored = invoke(&store)?;
    if stored.get("ok") == Some(&Value::Bool(true)) {
        stored["model_calls"] = json!(calls);
        stored["model_failures"] = json!(failures);
        stored["prompt_tokens"] = json!(prompt_tokens);
        stored["completion_tokens"] = json!(completion_tokens);
    }
    Ok(stored)
}

/// A connection whose collection file was deleted by hand would attach its base URL and
/// credential to a different API imported under the same name.
fn import(name: &str, source: Vec<String>) -> Result<Value, &'static str> {
    let request = request(ToolsCommand::Import {
        name: name.to_owned(),
        source,
    })?;
    let collection = crate::tool_catalog_directory()?.join(format!("{name}.json.gz"));
    if crate::http_read::connection_exists(name)? && std::fs::symlink_metadata(collection).is_err()
    {
        return Err("http_connection_without_collection");
    }
    invoke(&request)
}

/// The connection goes first: a connection left without its collection would attach its
/// base URL and credential to a different API later imported under the same name.
fn remove(name: &str) -> Result<Value, &'static str> {
    let request = request(ToolsCommand::Remove {
        name: name.to_owned(),
    })?;
    let connection = crate::http_read::disconnect(name)?;
    let collection = invoke(&request).unwrap_or_else(|code| json!({"ok": false, "error": code}));
    let missing = collection.get("error") == Some(&json!("collection_not_found"));
    if collection.get("ok") != Some(&Value::Bool(true)) && !(missing && connection.is_some()) {
        let mut failure = collection;
        match connection {
            Some(removed) => failure.as_object_mut().expect("object").extend(removed),
            None => failure["connection_removed"] = json!(false),
        }
        return Ok(failure);
    }
    let mut result = json!({"ok": true, "collection": name, "collection_removed": !missing});
    match connection {
        Some(removed) => result.as_object_mut().expect("object").extend(removed),
        None => result["connection_removed"] = json!(false),
    }
    Ok(result)
}

fn rebuild(name: Option<String>, all: bool) -> Result<Value, &'static str> {
    let one = |name: String| {
        request(ToolsCommand::Rebuild {
            name: Some(name),
            all: false,
        })
        .and_then(|request| invoke(&request))
    };
    if !all {
        return one(name.ok_or("invalid_collection_name")?);
    }
    let root = crate::tool_catalog_directory()?;
    let mut names = Vec::new();
    if root.exists() {
        for entry in std::fs::read_dir(&root).map_err(|_| "collection_directory_unavailable")? {
            let file = entry
                .map_err(|_| "collection_directory_unavailable")?
                .file_name();
            if let Some(name) = file.to_str().and_then(|file| file.strip_suffix(".json.gz")) {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    let mut ok = true;
    let mut results = Vec::new();
    for name in names {
        let result = one(name.clone()).unwrap_or_else(|code| json!({"ok": false, "error": code}));
        ok &= result.get("ok") == Some(&Value::Bool(true));
        results.push(json!({"collection": name, "result": result}));
        if INTERRUPTED.load(Ordering::Relaxed) {
            break;
        }
    }
    Ok(json!({"ok": ok, "collections": results}))
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
    if encoded.len() > MAX_REQUEST {
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
