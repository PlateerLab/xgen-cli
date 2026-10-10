use std::fmt;
use std::io::Write as _;

use serde_json::{Value, json};
use tempfile::NamedTempFile;
use url::Url;
use xgen_domain::InstanceBinding;
use xgen_policy::ResourceResolutionFailure;
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure,
    AdapterPrepareRequest, AdapterReconcileRequest, AdapterReconciliationInconclusiveReason,
    AdapterReconciliationObservation, AdapterToolOutput, EffectAdapter, PreparedAdapterInvocation,
};

use crate::execution::{canonical_output_digest, parse_prepared};
use crate::{ProcessExecuteVerifier, ProcessWorkspace};

pub const WEB_SEARCH_CAPABILITY_ID: &str = "xgen.web/search";
pub const WEB_SEARCH_CONTRACT_VERSION: &str = "1.0.0";
pub const WEB_SEARCH_EXECUTABLE_ID: &str = "openserp";
pub const WEB_SEARCH_SCOPE: &str = "web.search";
const MAX_QUERY_BYTES: usize = 1024;
const MAX_QUERY_CHARS: usize = 512;
const MAX_RESULTS: u64 = 5;
const MAX_OUTPUT_BYTES: usize = 32768;

/// Preserve the exact query as its permission resource; never rewrite it into a URL or path.
///
/// # Errors
/// Rejects empty, oversized, whitespace-only, or control-bearing queries without echoing them.
pub fn resolve_web_search_query(query: &str) -> Result<String, ResourceResolutionFailure> {
    valid_query(query)
        .then(|| query.to_owned())
        .ok_or(ResourceResolutionFailure::InvalidResource)
}

fn valid_query(query: &str) -> bool {
    !query.trim().is_empty()
        && query.len() <= MAX_QUERY_BYTES
        && query.chars().count() <= MAX_QUERY_CHARS
        && !query.chars().any(char::is_control)
}

fn parse_input(input: &Value) -> Option<(&str, u64)> {
    let object = input.as_object().filter(|object| object.len() == 2)?;
    let query = object
        .get("query")?
        .as_str()
        .filter(|query| valid_query(query))?;
    let maximum = object
        .get("maxResults")?
        .as_u64()
        .filter(|count| (1..=MAX_RESULTS).contains(count))?;
    Some((query, maximum))
}

/// Typed `OpenSERP` adapter reusing the catalogued, bounded, shell-free process executor.
#[derive(Clone)]
pub struct WebSearchAdapter {
    workspace: ProcessWorkspace,
}

impl fmt::Debug for WebSearchAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebSearchAdapter(<catalogued/redacted>)")
    }
}

impl WebSearchAdapter {
    pub(crate) const fn new(workspace: ProcessWorkspace) -> Self {
        Self { workspace }
    }

    #[must_use]
    pub fn binding(&self) -> InstanceBinding {
        let mut binding = self.workspace.binding();
        binding.operation_ref = Some("webSearch-v1-openserp-fixed-config".to_owned());
        binding
    }

    #[must_use]
    pub fn verifier(&self) -> ProcessExecuteVerifier {
        ProcessExecuteVerifier::web_search(self.binding())
    }

    #[must_use]
    pub fn accepts_normalized_material(&self, input: &Value) -> bool {
        parse_input(input).is_some()
    }

    fn prepare_input(
        &self,
        input: &Value,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        let (query, maximum) = parse_input(input).ok_or(AdapterPrepareFailure::InvalidMaterial)?;
        // An explicit private config prevents untrusted workspace config.yaml from affecting search.
        let mut configuration = tempfile::Builder::new()
            .prefix("xgen-web-search-")
            .suffix(".yaml")
            .tempfile()
            .map_err(|_| AdapterPrepareFailure::ResourceUnavailable)?;
        configuration
            .write_all(b"{}\n")
            .map_err(|_| AdapterPrepareFailure::ResourceUnavailable)?;
        let config_path = configuration
            .path()
            .to_str()
            .ok_or(AdapterPrepareFailure::ResourceUnavailable)?;
        let args = search_argv(query, maximum, config_path);
        let process = parse_prepared(
            &json!({
                "executable": format!("process:{}/executables/{WEB_SEARCH_EXECUTABLE_ID}", self.workspace.workspace_id.as_str()),
                "args": args, "cwd": ".", "env": {}, "timeoutMs": 30000, "maxOutputBytes": MAX_OUTPUT_BYTES
            }),
            &self.workspace,
        )?;
        Ok(Box::new(PreparedWebSearch {
            process,
            query: query.to_owned(),
            maximum,
            _configuration: configuration,
        }))
    }
}

fn search_argv(query: &str, maximum: u64, config_path: &str) -> Vec<String> {
    [
        "search",
        "--config",
        config_path,
        "--limit",
        &maximum.to_string(),
        "--timeout",
        "8",
        "--search-timeout",
        "12",
        "--max_retries",
        "0",
        "--format",
        "json",
        "--leakless",
        "--",
        "duckduckgo",
        query,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

impl EffectAdapter for WebSearchAdapter {
    fn prepare(
        &mut self,
        request: AdapterPrepareRequest<'_>,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        let intent = request.intent();
        let instance = request.instance();
        if intent.invocation.capability_id != WEB_SEARCH_CAPABILITY_ID
            || intent.invocation.contract_version != WEB_SEARCH_CONTRACT_VERSION
            || instance.definition.capability_id != WEB_SEARCH_CAPABILITY_ID
            || instance.definition.contract_version != WEB_SEARCH_CONTRACT_VERSION
            || intent.invocation.instance_id != instance.instance_id
            || instance.binding != self.binding()
            || !crate::execution::supports_instance_features(&instance.features)
            || intent.effect_class != xgen_workgraph::EffectClass::NonIdempotent
            || intent.idempotency_key.as_deref().is_none_or(str::is_empty)
        {
            return Err(AdapterPrepareFailure::UnsupportedProtocol);
        }
        self.prepare_input(request.normalized_arguments())
    }

    fn reconcile(
        &mut self,
        _request: AdapterReconcileRequest<'_>,
    ) -> AdapterReconciliationObservation {
        AdapterReconciliationObservation::Inconclusive {
            reason: AdapterReconciliationInconclusiveReason::StableKeyUnsupported,
        }
    }
}

struct PreparedWebSearch {
    process: Box<dyn PreparedAdapterInvocation>,
    query: String,
    maximum: u64,
    _configuration: NamedTempFile,
}

impl PreparedAdapterInvocation for PreparedWebSearch {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        let observation = self.process.execute();
        let AdapterExecutionObservation::SucceededWithOutput { output, .. } = observation else {
            return observation;
        };
        let value = normalize_process_output(&self.query, self.maximum, output.as_value());
        let digest = canonical_output_digest(&value).expect("finite bounded search output");
        AdapterExecutionObservation::SucceededWithOutput {
            evidence_digest: AdapterEvidenceDigest::new(digest).expect("canonical SHA-256"),
            output: AdapterToolOutput::new(value),
        }
    }
}

fn result_rows(payload: &Value, maximum: u64) -> Option<Vec<Value>> {
    let rows = payload.get("results")?.as_array()?;
    if rows.len() > usize::try_from(maximum).ok()? {
        return None;
    }
    rows.iter()
        .map(|row| {
            let title = row.get("title")?.as_str()?;
            let address = row.get("url")?.as_str()?;
            let snippet = row.get("snippet").map_or(Some(""), Value::as_str)?;
            let parsed = Url::parse(address).ok()?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || address.chars().any(char::is_control)
                || address.len() > 4096
                || title.chars().count() > 512
                || snippet.chars().count() > 2048
            {
                return None;
            }
            Some(json!({"title": title, "url": address, "snippet": snippet}))
        })
        .collect()
}

fn normalize_process_output(query: &str, maximum: u64, process: &Value) -> Value {
    let mut rows = Vec::new();
    let mut outcome = match process["outcome"].as_str() {
        Some("timed_out") => "timed_out",
        Some("launch_failed") => "launch_failed",
        _ if process["success"] != true => "backend_failed",
        _ if process["stdoutTruncated"] == true => "output_truncated",
        _ => match process["stdout"]
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|payload| result_rows(&payload, maximum))
        {
            None => "invalid_response",
            Some(results) => {
                rows = results;
                if rows.is_empty() {
                    "no_results"
                } else {
                    "results"
                }
            }
        },
    };
    let value = json!({"outcome": outcome, "query": query, "results": rows,
        "exitCode": process["exitCode"], "durationMs": process["durationMs"]});
    if serde_jcs::to_vec(&value).is_ok_and(|bytes| bytes.len() <= MAX_OUTPUT_BYTES) {
        return value;
    }
    outcome = "output_truncated";
    json!({"outcome": outcome, "query": query, "results": [],
        "exitCode": process["exitCode"], "durationMs": process["durationMs"]})
}

pub(crate) fn inspect_search_output(
    output: &Value,
    expected_digest: Option<&str>,
) -> Result<crate::verifier::InspectedOutput, ()> {
    let object = output
        .as_object()
        .filter(|object| object.len() == 5)
        .ok_or(())?;
    object
        .get("query")
        .and_then(Value::as_str)
        .filter(|query| valid_query(query))
        .ok_or(())?;
    let outcome = object.get("outcome").and_then(Value::as_str).ok_or(())?;
    let rows = result_rows(output, MAX_RESULTS).ok_or(())?;
    if Some(&rows) != output["results"].as_array() {
        return Err(());
    }
    let exit_code = match &output["exitCode"] {
        Value::Null => None,
        value => Some(
            value
                .as_i64()
                .and_then(|code| i32::try_from(code).ok())
                .ok_or(())?,
        ),
    };
    match outcome {
        "results" if !rows.is_empty() && exit_code == Some(0) => {}
        "no_results" | "invalid_response" | "output_truncated"
            if rows.is_empty() && exit_code == Some(0) => {}
        "backend_failed" if rows.is_empty() && exit_code != Some(0) => {}
        "timed_out" | "launch_failed" if rows.is_empty() && exit_code.is_none() => {}
        _ => return Err(()),
    }
    output["durationMs"]
        .as_u64()
        .filter(|value| *value <= crate::execution::MAX_RESULT_DURATION_MS)
        .ok_or(())?;
    let canonical = serde_jcs::to_vec(output).map_err(|_| ())?;
    if canonical.len() > MAX_OUTPUT_BYTES {
        return Err(());
    }
    let digest = crate::execution::sha256_digest(&canonical);
    if expected_digest.is_some_and(|expected| expected != digest) {
        return Err(());
    }
    Ok(crate::verifier::InspectedOutput {
        digest,
        canonical_size_bytes: u64::try_from(canonical.len()).map_err(|_| ())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_typed_bounded_and_unicode_safe() {
        for query in [
            "Rust process docs",
            "파이썬 ast 공식 문서",
            "--help;$(touch marker)",
        ] {
            assert!(parse_input(&json!({"query": query, "maxResults": 3})).is_some());
            assert_eq!(resolve_web_search_query(query).unwrap(), query);
            let args = search_argv(query, 3, "/host/config.yaml");
            assert_eq!(&args[args.len() - 3..], &["--", "duckduckgo", query]);
        }
        for input in [
            json!({"query":"x","maxResults":0}),
            json!({"query":"x","maxResults":6}),
            json!({"query":" ","maxResults":1}),
            json!({"query":"x\u{1b}","maxResults":1}),
            json!({"query":"x","maxResults":1,"args":[]}),
            json!({"query":"한".repeat(400),"maxResults":1}),
        ] {
            assert!(parse_input(&input).is_none());
        }
    }

    #[test]
    fn sources_preserve_order_and_fail_closed_without_partial_json() {
        for title in ["English docs", "한국어 문서"] {
            let rows =
                json!([{"title":title,"url":"https://example.org/one","snippet":"source text"}]);
            let process = json!({"outcome":"exited","success":true,"exitCode":0,"stdout":json!({"results":rows}).to_string(),"stdoutTruncated":false,"durationMs":5});
            let output = normalize_process_output("query", 2, &process);
            assert_eq!(output["results"], rows);
            assert!(inspect_search_output(&output, None).is_ok());
            let mut broken = process.clone();
            broken["stdoutTruncated"] = json!(true);
            assert_eq!(
                normalize_process_output("query", 2, &broken)["outcome"],
                "output_truncated"
            );
            broken["stdoutTruncated"] = json!(false);
            broken["stdout"] = json!("not JSON");
            assert_eq!(
                normalize_process_output("query", 2, &broken)["outcome"],
                "invalid_response"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn backend_executes_literal_queries_with_host_config_and_bounded_failures() {
        use crate::{ExecutableCatalog, ProcessEnvironment, ProcessWorkspaceId};
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("openserp");
        // A fixture backend verifies the same argv and private config used in production.
        std::fs::write(&executable, r#"#!/bin/sh
[ "$1" = search ] && [ "$2" = --config ] || exit 70
[ "$(cat "$3")" = '{}' ] || exit 71
shift 3
[ "$1" = --limit ] || exit 72
shift 11
[ "$1" = -- ] && [ "$2" = duckduckgo ] && [ "$#" = 3 ] || exit 73
case "$3" in
  '--help;$(touch marker)') printf '%s' '{"results":[{"title":"English docs","url":"https://example.org/en","snippet":"observed"}]}' ;;
  '한국어 검색') printf '%s' '{"results":[{"title":"한국어 문서","url":"https://example.org/ko"}]}' ;;
  failure) exit 7 ;;
  malformed) printf '%s' 'not JSON' ;;
  *) exit 74 ;;
esac
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(root.path().join("config.yaml"), "untrusted: true\n").unwrap();
        let catalog = ExecutableCatalog::from_paths([("openserp", executable)]).unwrap();
        let workspace = ProcessWorkspace::open_ambient(
            root.path(),
            ProcessWorkspaceId::new("fixture").unwrap(),
            catalog,
            ProcessEnvironment::new(BTreeMap::new()).unwrap(),
        )
        .unwrap();
        let adapter = workspace.web_search_adapter().unwrap();
        for (query, expected) in [
            ("--help;$(touch marker)", "results"),
            ("한국어 검색", "results"),
            ("failure", "backend_failed"),
            ("malformed", "invalid_response"),
        ] {
            let prepared = adapter
                .prepare_input(&json!({"query":query,"maxResults":2}))
                .unwrap();
            let AdapterExecutionObservation::SucceededWithOutput {
                output,
                evidence_digest,
            } = prepared.execute()
            else {
                panic!("fixture execution must be observed")
            };
            assert_eq!(output.as_value()["query"], query);
            assert_eq!(output.as_value()["outcome"], expected, "{query}");
            assert!(
                inspect_search_output(output.as_value(), Some(evidence_digest.as_str())).is_ok()
            );
        }
        assert!(!root.path().join("marker").exists());
    }

    #[test]
    fn failure_observations_are_durable_and_success_claims_cannot_be_forged() {
        for (process_outcome, exit_code, expected) in [
            ("exited", json!(7), "backend_failed"),
            ("exited", Value::Null, "backend_failed"),
            ("timed_out", Value::Null, "timed_out"),
            ("launch_failed", Value::Null, "launch_failed"),
        ] {
            let process = json!({"outcome":process_outcome,"success":false,"exitCode":exit_code,"stdout":"untrusted failure text","durationMs":5});
            let output = normalize_process_output("query", 2, &process);
            assert_eq!(output["outcome"], expected);
            assert_eq!(output["results"], json!([]));
            assert!(inspect_search_output(&output, None).is_ok());
            let mut forged = output.clone();
            forged["results"] =
                json!([{"title":"invented","url":"https://example.org","snippet":""}]);
            assert!(inspect_search_output(&forged, None).is_err());
        }
    }

    #[test]
    fn result_urls_and_shape_are_untrusted() {
        for url in [
            "file:///etc/passwd",
            "https://user:pass@example.org",
            "javascript:alert(1)",
            "https://example.org\n",
        ] {
            assert!(result_rows(&json!({"results":[{"title":"x","url":url}]}), 1).is_none());
        }
        assert!(result_rows(&json!({"results":null}), 1).is_none());
        assert!(result_rows(&json!({"results":[{"title":"x","url":"https://example.org"},{"title":"y","url":"https://example.org"}]}), 1).is_none());
    }
}
