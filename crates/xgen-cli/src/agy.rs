//! Experimental official-CLI proposal transport. Native activity is rejected after observation.
#![allow(clippy::result_unit_err)] // Internal configuration failures are deliberately redacted.
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

#[cfg(windows)]
use process_wrap::std::JobObject;
#[cfg(unix)]
use process_wrap::std::ProcessGroup;
use process_wrap::std::{ChildWrapper, CommandWrap};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;
use xgen_provider_openai::parse_unique_json;
use xgen_runtime::PlannerPortFailure;

pub const DEFAULT_MODEL: &str = "gemini-3.8-flash-low";
const AGENT: &str = include_str!("agy-agent.md");
const REVISION: &str = "xgen.agy-transport/v1";
const MAX_STDOUT: usize = 131_072;
const MAX_STDERR: usize = 32_768;
static CANCELLED: AtomicBool = AtomicBool::new(false);
type Child = Arc<Mutex<Box<dyn ChildWrapper>>>;
type WeakChild = Weak<Mutex<Box<dyn ChildWrapper>>>;
static ACTIVE: Mutex<Vec<WeakChild>> = Mutex::new(Vec::new());
type Directory = Arc<Mutex<Option<TempDir>>>;
type WeakDirectory = Weak<Mutex<Option<TempDir>>>;
static DIRECTORIES: Mutex<Vec<WeakDirectory>> = Mutex::new(Vec::new());

pub fn request_cancellation() {
    CANCELLED.store(true, Ordering::SeqCst);
}
pub fn clear_cancellation() {
    CANCELLED.store(false, Ordering::SeqCst);
}

pub fn kill_active() {
    let children: Vec<_> = ACTIVE
        .lock()
        .map(|list| list.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();
    for child in children {
        stop(&child);
    }
    let directories: Vec<_> = DIRECTORIES
        .lock()
        .map(|list| list.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();
    for directory in directories {
        if let Ok(mut owned) = directory.lock() {
            drop(owned.take());
        }
    }
}

fn stop(child: &Child) {
    if let Ok(mut child) = child.lock() {
        let _ = child.start_kill();
        #[cfg(not(windows))]
        let _ = child.wait();
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        stop(&self.0);
    }
}

/// Non-secret transport descriptor stored in the existing profile endpoint slot.
#[derive(Clone, Debug)]
pub struct Spec {
    executable: PathBuf,
    sha256: String,
}
impl Spec {
    pub fn create(executable: &Path) -> Result<Self, ()> {
        let executable = executable.canonicalize().map_err(|_| ())?;
        let sha256 = hash_file(&executable)?;
        Ok(Self { executable, sha256 })
    }
    pub fn parse(value: &str) -> Result<Self, ()> {
        let url = Url::parse(value).map_err(|_| ())?;
        if url.scheme() != "agy" || url.host_str().is_some() || url.fragment().is_some() {
            return Err(());
        }
        let query: Vec<_> = url.query_pairs().collect();
        if query.len() != 1 || query[0].0 != "sha256" {
            return Err(());
        }
        let sha256 = query[0].1.to_string();
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(());
        }
        let mut file = Url::parse(&value.replacen("agy:", "file:", 1)).map_err(|_| ())?;
        file.set_query(None);
        let executable = file.to_file_path()?;
        if !executable.is_absolute() {
            return Err(());
        }
        Ok(Self { executable, sha256 })
    }
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    pub fn uri(&self) -> Result<String, ()> {
        let mut url = Url::from_file_path(&self.executable)?;
        url.query_pairs_mut().append_pair("sha256", &self.sha256);
        Ok(url.as_str().replacen("file:", "agy:", 1))
    }
    pub fn verify(&self) -> Result<(), ()> {
        if !cfg!(target_os = "linux") {
            return Err(());
        }
        if hash_file(&self.executable)? != self.sha256 {
            return Err(());
        }
        if std::env::var_os("HOME").is_some_and(|home| {
            PathBuf::from(home)
                .join(".gemini/config/hooks.json")
                .exists()
        }) {
            return Err(());
        }
        Ok(())
    }
    /// Stable synthetic endpoint binds executable and the generated agent/settings policy.
    pub fn endpoint(&self) -> Result<String, ()> {
        let policy = json!([
            REVISION,
            self.uri()?,
            AGENT,
            settings("${WORKSPACE}"),
            MAX_STDOUT,
            MAX_STDERR,
            "agy-uncached-input/v1",
            "allowlisted-environment/v1",
            prompt_limit()
        ]);
        Ok(format!(
            "https://agy.invalid/v1/{}",
            hash(&serde_json::to_vec(&policy).map_err(|_| ())?)
        ))
    }
}
#[must_use]
pub fn is_agy(value: &str) -> bool {
    value.starts_with("agy:")
}
pub fn endpoint(value: &str) -> Result<String, ()> {
    if is_agy(value) {
        Spec::parse(value)?.endpoint()
    } else {
        Ok(value.to_owned())
    }
}
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hash_file(path: &Path) -> Result<String, ()> {
    let mut file = fs::File::open(path).map_err(|_| ())?;
    if !file.metadata().map_err(|_| ())?.is_file() {
        return Err(());
    }
    let mut sha = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(|_| ())?;
        if count == 0 {
            break;
        }
        sha.update(&buffer[..count]);
    }
    Ok(hex(&sha.finalize()))
}
fn settings(workspace: &str) -> Value {
    json!({"trustedWorkspaces":[workspace],"agentMode":"","toolPermission":"strict",
        "allowNonWorkspaceAccess":false,"disableSlashCommands":true,"useG1Credits":false,"notifications":false,
        "permissions":{"deny":["command(*)","read_file(*)","write_file(*)","read_url(*)","execute_url(*)","mcp(*)"]}})
}
fn prompt_limit() -> usize {
    if cfg!(windows) { 24_000 } else { 100_000 }
}

pub struct Transport {
    spec: Spec,
    _directory: Directory,
    workspace: PathBuf,
    executable: PathBuf,
    settings: PathBuf,
    model: String,
    timeout: Duration,
    closing: &'static str,
}
const PROPOSAL_CLOSING: &str = "Return exactly one proposal JSON object. No Markdown fences or commentary. All actions remain proposals for the host. Never call your own tools or access the environment.";
const TASK_CLOSING: &str = "Return exactly one JSON object as instructed. No Markdown fences or commentary. Never call your own tools or access the environment.";
impl Transport {
    pub fn new(spec: Spec, model: &str, timeout: Duration) -> Result<Self, ()> {
        spec.verify()?;
        let mut registered = DIRECTORIES.lock().map_err(|_| ())?;
        let directory = tempfile::Builder::new()
            .prefix("xgen-agy-")
            .tempdir()
            .map_err(|_| ())?;
        let root = directory.path().to_owned();
        let directory = Arc::new(Mutex::new(Some(directory)));
        registered.retain(|dir| dir.strong_count() > 0);
        registered.push(Arc::downgrade(&directory));
        drop(registered);
        let workspace = root.join("workspace");
        fs::create_dir_all(workspace.join(".agents/agents")).map_err(|_| ())?;
        fs::write(workspace.join(".agents/agents/xgen-planner-only.md"), AGENT).map_err(|_| ())?;
        let settings_path = root.join("settings.json");
        fs::write(
            &settings_path,
            serde_json::to_vec(&settings(workspace.to_str().ok_or(())?)).map_err(|_| ())?,
        )
        .map_err(|_| ())?;
        let executable = root.join(if cfg!(windows) { "agy.exe" } else { "agy" });
        fs::copy(&spec.executable, &executable).map_err(|_| ())?;
        if hash_file(&executable)? != spec.sha256 {
            return Err(());
        }
        Ok(Self {
            spec,
            _directory: directory,
            workspace,
            executable,
            settings: settings_path,
            model: model.to_owned(),
            timeout,
            closing: PROPOSAL_CLOSING,
        })
    }
    /// Close prompts for a host JSON task (summary enrichment) instead of a plan proposal.
    #[must_use]
    pub fn for_json_task(mut self) -> Self {
        self.closing = TASK_CLOSING;
        self
    }
    fn unchanged(&self) -> Result<(), ()> {
        self.spec.verify()?;
        if hash_file(&self.executable)? != self.spec.sha256 {
            return Err(());
        }
        let expected =
            serde_json::to_vec(&settings(self.workspace.to_str().ok_or(())?)).map_err(|_| ())?;
        for (dir, expected_name) in [
            (&self.workspace, ".agents"),
            (&self.workspace.join(".agents"), "agents"),
            (
                &self.workspace.join(".agents/agents"),
                "xgen-planner-only.md",
            ),
        ] {
            if fs::symlink_metadata(dir)
                .map_err(|_| ())?
                .file_type()
                .is_symlink()
            {
                return Err(());
            }
            let entries: Vec<_> = fs::read_dir(dir)
                .map_err(|_| ())?
                .collect::<Result<_, _>>()
                .map_err(|_| ())?;
            if entries.len() != 1
                || entries[0].file_name() != expected_name
                || entries[0].file_type().map_err(|_| ())?.is_symlink()
            {
                return Err(());
            }
        }
        if !fs::symlink_metadata(&self.settings)
            .map_err(|_| ())?
            .file_type()
            .is_file()
        {
            return Err(());
        }
        if fs::read(&self.settings).map_err(|_| ())? != expected {
            return Err(());
        }
        if fs::read(self.workspace.join(".agents/agents/xgen-planner-only.md")).map_err(|_| ())?
            != AGENT.as_bytes()
        {
            return Err(());
        }
        Ok(())
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.current_dir(&self.workspace);
        isolate_environment(&mut command);
        command.args(["--settings"]).arg(&self.settings);
        command
    }
    /// `unchanged`, leaving a fixed diagnostic when the pinned layout no longer holds.
    fn pinned(&self) -> Result<(), ()> {
        self.unchanged()
            .inspect_err(|()| eprintln!("XGEN_AGY code=pin_changed"))
    }
    pub fn models(&self) -> Result<Vec<String>, crate::ModelCheckError> {
        self.pinned()
            .map_err(|()| crate::ModelCheckError::Configuration)?;
        let mut command = self.command();
        command.arg("models");
        let output = run(command, Duration::from_secs(30), None).map_err(|error| match error {
            PlannerPortFailure::ProviderRejected => crate::ModelCheckError::AuthenticationRejected,
            PlannerPortFailure::Timeout => crate::ModelCheckError::Timeout,
            _ => crate::ModelCheckError::Unavailable,
        })?;
        self.pinned()
            .map_err(|()| crate::ModelCheckError::Configuration)?;
        let text =
            std::str::from_utf8(&output).map_err(|_| crate::ModelCheckError::InvalidResponse)?;
        let mut models = BTreeSet::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let id = line
                .split_whitespace()
                .next()
                .ok_or(crate::ModelCheckError::InvalidResponse)?;
            if id.len() > 256
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._/:".contains(&b))
                || !models.insert(id.to_owned())
            {
                return Err(crate::ModelCheckError::InvalidResponse);
            }
        }
        if models.is_empty() || models.len() > 512 {
            return Err(crate::ModelCheckError::InvalidResponse);
        }
        Ok(models.into_iter().collect())
    }
    pub fn send(
        &mut self,
        body: &[u8],
        max_response_bytes: usize,
    ) -> Result<Vec<u8>, PlannerPortFailure> {
        self.pinned()
            .map_err(|()| PlannerPortFailure::Unavailable)?;
        let request = parse_unique_json(body, 64)?;
        if request["model"].as_str() != Some(self.model.as_str()) {
            return Err(PlannerPortFailure::ProviderLimit);
        }
        let system = request["messages"][0]["content"]
            .as_str()
            .ok_or(PlannerPortFailure::InvalidResponse)?;
        let user = request["messages"][1]["content"]
            .as_str()
            .ok_or(PlannerPortFailure::InvalidResponse)?;
        let prompt = format!(
            "HOST SYSTEM INSTRUCTIONS:\n{system}\nHOST REQUEST AND OBSERVATIONS:\n{user}\n{}",
            self.closing
        );
        if prompt.len() > prompt_limit() {
            return Err(PlannerPortFailure::RequestTooLarge);
        }
        let mut command = self.command();
        command
            .args([
                "--agent",
                "xgen-planner-only",
                "--model",
                &self.model,
                "--disable-slash-commands",
                "--output-format",
                "stream-json",
                "--print-timeout",
            ])
            .arg(format!("{}s", self.timeout.as_secs()))
            .args(["-p", &prompt]);
        let raw = run(command, self.timeout, Some(&self.model))?;
        self.pinned()
            .map_err(|()| PlannerPortFailure::Unavailable)?;
        let events = decode_events(&raw)?;
        let finals: Vec<_> = events.iter().filter(|v| v["event"] == "result").collect();
        if events.iter().filter(|v| v["event"] == "init").count() != 1
            || finals.len() != 1
            || finals[0]["result"]["status"] != "SUCCESS"
        {
            return Err(PlannerPortFailure::InvalidResponse);
        }
        let result = &finals[0]["result"];
        let response = result["response"]
            .as_str()
            .ok_or(PlannerPortFailure::InvalidResponse)?;
        parse_unique_json(response.as_bytes(), 64)?;
        let mut envelope = json!({"model":self.model,"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":response}}]});
        if let Some(usage) = normalized_usage(&result["usage"]) {
            envelope["usage"] = usage;
        }
        let encoded =
            serde_json::to_vec(&envelope).map_err(|_| PlannerPortFailure::InvalidResponse)?;
        if encoded.len() > max_response_bytes {
            return Err(PlannerPortFailure::ProviderLimit);
        }
        Ok(encoded)
    }
}

fn normalized_usage(value: &Value) -> Option<Value> {
    let input = value["input_tokens"].as_u64()?;
    let output = value["output_tokens"].as_u64()?;
    let cache = value["cache_read_tokens"].as_u64()?;
    if value["total_tokens"].as_u64()? != input.checked_add(output)? {
        return None;
    }
    let prompt = input.checked_add(cache)?;
    let mut usage = json!({"prompt_tokens":prompt,"completion_tokens":output,"total_tokens":prompt.checked_add(output)?,
                "prompt_tokens_details":{"cached_tokens":cache}});
    if let Some(reasoning) = value["thinking_tokens"].as_u64().filter(|v| *v <= output) {
        usage["completion_tokens_details"] = json!({"reasoning_tokens":reasoning});
    }
    Some(usage)
}
fn decode_events(raw: &[u8]) -> Result<Vec<Value>, PlannerPortFailure> {
    raw.split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .map(|line| parse_unique_json(line, 64))
        .collect()
}
fn observe(line: &[u8], model: &str) -> Result<(), PlannerPortFailure> {
    let event = parse_unique_json(line, 64)?;
    if event["event"] == "init"
        && (event["init"]["agent"] != "xgen-planner-only" || event["init"]["model"] != model)
    {
        return Err(PlannerPortFailure::Unavailable);
    }
    let step = &event["step_update"];
    if step["step_type"] == "tool"
        || step.get("tool_info").is_some_and(|v| !v.is_null())
        || step.get("subagent_info").is_some_and(|v| !v.is_null())
    {
        eprintln!("XGEN_AGY code=native_tool_observed");
        return Err(PlannerPortFailure::Unavailable);
    }
    Ok(())
}
struct Packet {
    stdout: bool,
    bytes: Vec<u8>,
    failed: bool,
}
fn capture(
    mut reader: impl Read + Send + 'static,
    stdout: bool,
    sender: mpsc::SyncSender<Packet>,
) -> Result<(), PlannerPortFailure> {
    std::thread::Builder::new()
        .name("xgen-agy-pipe".to_owned())
        .spawn(move || {
            let mut remaining = (if stdout { MAX_STDOUT } else { MAX_STDERR }) + 1;
            while remaining > 0 {
                let mut chunk = vec![0; 4096.min(remaining)];
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Err(_) => {
                        let _ = sender.send(Packet {
                            stdout,
                            bytes: Vec::new(),
                            failed: true,
                        });
                        break;
                    }
                    Ok(count) => {
                        chunk.truncate(count);
                        remaining -= count;
                        if sender
                            .send(Packet {
                                stdout,
                                bytes: chunk,
                                failed: false,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|_| PlannerPortFailure::Unavailable)?;
    Ok(())
}
#[allow(clippy::too_many_lines)] // Keep the bounded stream/child lifecycle in one audit boundary.
fn run(
    mut command: Command,
    timeout: Duration,
    model: Option<&str>,
) -> Result<Vec<u8>, PlannerPortFailure> {
    if CANCELLED.load(Ordering::SeqCst) {
        return Err(PlannerPortFailure::Unavailable);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(JobObject);
    let mut active = ACTIVE.lock().map_err(|_| PlannerPortFailure::Unavailable)?;
    let child = Arc::new(Mutex::new(
        wrapped
            .spawn()
            .map_err(|_| PlannerPortFailure::Unavailable)?,
    ));
    let _owned = OwnedChild(child.clone());
    let stdout = child
        .lock()
        .map_err(|_| PlannerPortFailure::Unavailable)?
        .stdout()
        .take()
        .ok_or(PlannerPortFailure::Unavailable)?;
    let stderr = child
        .lock()
        .map_err(|_| PlannerPortFailure::Unavailable)?
        .stderr()
        .take()
        .ok_or(PlannerPortFailure::Unavailable)?;
    active.retain(|child| child.strong_count() > 0);
    active.push(Arc::downgrade(&child));
    drop(active);
    let (sender, receiver) = mpsc::sync_channel(8);
    capture(stdout, true, sender.clone())?;
    capture(stderr, false, sender)?;
    let started = Instant::now();
    let (mut output, mut pending, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    let mut status = None;
    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err(PlannerPortFailure::Unavailable);
        }
        if started.elapsed() >= timeout {
            return Err(PlannerPortFailure::Timeout);
        }
        if status.is_none() {
            status = child
                .lock()
                .map_err(|_| PlannerPortFailure::Unavailable)?
                .try_wait()
                .map_err(|_| PlannerPortFailure::Unavailable)?;
            if status.is_some() {
                stop(&child);
            }
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(packet) => {
                if packet.failed {
                    return Err(PlannerPortFailure::Unavailable);
                }
                if packet.stdout {
                    if output.len() + packet.bytes.len() > MAX_STDOUT {
                        return Err(PlannerPortFailure::ProviderLimit);
                    }
                    output.extend_from_slice(&packet.bytes);
                    if let Some(model) = model {
                        pending.extend_from_slice(&packet.bytes);
                        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<_> = pending.drain(..=end).collect();
                            if !line.iter().all(u8::is_ascii_whitespace) {
                                observe(&line, model)?;
                            }
                        }
                    }
                } else {
                    errors.extend_from_slice(&packet.bytes);
                    if errors.len() > MAX_STDERR {
                        return Err(PlannerPortFailure::ProviderLimit);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(model) = model
                    && !pending.iter().all(u8::is_ascii_whitespace)
                {
                    observe(&pending, model)?;
                }
                if let Some(status) = status {
                    if status.success() {
                        return Ok(output);
                    }
                    let errors = String::from_utf8_lossy(&errors);
                    if errors.contains("Please sign in") {
                        return Err(PlannerPortFailure::ProviderRejected);
                    }
                    // Only a fixed class leaves the process; agy's own text stays private.
                    let class = exit_class(&errors);
                    eprintln!(
                        "XGEN_AGY code=agy_exit status={} class={class}",
                        status
                            .code()
                            .map_or_else(|| "signal".to_owned(), |code| code.to_string())
                    );
                    return Err(PlannerPortFailure::Unavailable);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Pass only the variables agy needs, and keep it from updating itself.
///
/// agy's background updater replaces the running executable, which here is the pinned copy:
/// the next pin check then fails and every later call in the process is refused. Observed on
/// the first call after agy's update-check interval (2026-11-28, 2/2 with and 0/2 without
/// this variable; the value `1` does not disable it).
fn isolate_environment(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "LANG",
        "LC_ALL",
        "SYSTEMROOT",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
}

/// Coarse class of an unsuccessful agy exit, from its stderr.
fn exit_class(errors: &str) -> &'static str {
    let lowered = errors.to_ascii_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| lowered.contains(needle));
    if any(&[
        "quota",
        "rate limit",
        "resource exhausted",
        "resource_exhausted",
        "429",
        "too many requests",
    ]) {
        "rate_limited"
    } else if any(&[
        "unauthenticated",
        "unauthorized",
        "401",
        "403",
        "token",
        "credential",
        "login",
    ]) {
        "authentication"
    } else if any(&[
        "network",
        "connect",
        "dns",
        "timed out",
        "timeout",
        "unreachable",
        "503",
        "502",
        "unavailable",
    ]) {
        "network"
    } else if lowered.trim().is_empty() {
        "no_stderr"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agy_runs_without_inherited_variables_or_self_update() {
        let mut command = Command::new("agy");
        isolate_environment(&mut command);
        let envs: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .collect();
        assert!(
            envs.iter()
                .any(|(key, value)| *key == "AGY_CLI_DISABLE_AUTO_UPDATE"
                    && *value == Some(std::ffi::OsStr::new("true")))
        );
        assert!(envs.iter().all(|(key, _)| *key != "XGEN_OPENAI_API_KEY"));
    }

    #[test]
    fn exit_classes_are_fixed_and_never_echo_agy_text() {
        assert_eq!(
            exit_class("Error: RESOURCE_EXHAUSTED quota"),
            "rate_limited"
        );
        assert_eq!(exit_class("failed to refresh token"), "authentication");
        assert_eq!(
            exit_class("dial tcp: connect: network is unreachable"),
            "network"
        );
        assert_eq!(exit_class("  "), "no_stderr");
        assert_eq!(exit_class("panic: something else"), "other");
    }

    #[test]
    fn usage_adds_cache_once_and_never_fabricates_missing_counts() {
        for (input, cache, output) in [(278, 30214, 4), (2087, 7003, 109), (2789, 5234, 107)] {
            let value = json!({"input_tokens":input,"cache_read_tokens":cache,"output_tokens":output,
                               "thinking_tokens":output,"total_tokens":input+output});
            let normalized = normalized_usage(&value).unwrap();
            assert_eq!(normalized["prompt_tokens"], input + cache);
            assert_eq!(normalized["completion_tokens"], output);
            assert_eq!(normalized["total_tokens"], input + cache + output);
        }
        for value in [
            json!({}),
            json!({"input_tokens":true}),
            json!({"input_tokens":1,"cache_read_tokens":2,"output_tokens":3,"total_tokens":8}),
            json!({"input_tokens":u64::MAX,"cache_read_tokens":1,"output_tokens":0,"total_tokens":u64::MAX}),
        ] {
            assert!(normalized_usage(&value).is_none());
        }
    }
    #[test]
    fn native_subagent_identity_and_duplicate_events_fail_closed() {
        for value in [
            json!({"event":"step_update","step_update":{"subagent_info":{}}}),
            json!({"event":"step_update","step_update":{"tool_info":{"name":"unexpected"}}}),
            json!({"event":"init","init":{"model":"other","agent":"xgen-planner-only"}}),
        ] {
            assert!(observe(&serde_json::to_vec(&value).unwrap(), DEFAULT_MODEL).is_err());
        }
        assert!(observe(br#"{"event":"init","event":"result"}"#, DEFAULT_MODEL).is_err());
    }
}
