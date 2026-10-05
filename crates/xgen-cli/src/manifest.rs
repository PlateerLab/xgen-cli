use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use xgen_adapter_filesystem::WorkspaceId;
use xgen_workgraph::{AgentLoopBudget, ModelCallBudget};

const MANIFEST_FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_PROFILE_TEXT_BYTES: usize = 512;
const AUTHORITY_PREFIX: &str = "local:xgeny-cli-";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunManifest {
    record: RunManifestRecord,
    record_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunManifestRecord {
    format_version: u32,
    run_id: String,
    workspace_id: String,
    workspace_root_identity_profile: String,
    workspace_root_identity_digest: String,
    planner_id: String,
    model: String,
    tokenizer: String,
    request_profile_digest: String,
    model_data_boundary: ModelDataBoundary,
    allow_file_catalog_digest: String,
    local_execution_profile_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_fingerprints: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    tool_discovery_snapshots: crate::graph_discovery::Snapshots,
    budget: ManifestBudget,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    conversation_responses: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    final_response_schema: Option<SavedFinalResponseSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_checks: Option<Vec<xgen_provider_openai::CompletionCheck>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct SavedFinalResponseSchema(serde_json::Value);

impl std::fmt::Debug for SavedFinalResponseSchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted final response schema>")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModelDataBoundary {
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Keep every persisted field explicit about being a maximum.
pub(crate) struct ManifestBudget {
    pub(crate) max_model_turns: u32,
    pub(crate) max_model_calls: u32,
    pub(crate) max_planned_steps: u32,
    pub(crate) max_tool_calls: u32,
    pub(crate) max_context_bytes: u64,
}

impl Default for ManifestBudget {
    fn default() -> Self {
        Self {
            max_model_turns: 2,
            max_model_calls: 4,
            max_planned_steps: 1,
            max_tool_calls: 1,
            max_context_bytes: 512 * 1024,
        }
    }
}

/// Upper bound for a host-selected model turn budget; planned steps stay within the
/// planner's source-step bound.
pub const MAX_HOST_MODEL_TURNS: u32 = 4_096;

impl ManifestBudget {
    pub(crate) const fn workspace_discovery() -> Self {
        Self {
            max_model_turns: 8,
            max_model_calls: 16,
            max_planned_steps: 8,
            max_tool_calls: 8,
            max_context_bytes: 512 * 1024,
        }
    }

    /// Host-selected agent loop budget for workspace discovery, recorded in the Run manifest.
    ///
    /// Model calls, planned steps and tool calls keep the `workspace_discovery` ratio
    /// (2:1:1 per model turn). The context bound is unchanged. Resume reads the manifest, so a
    /// continued Run keeps the budget it was created with.
    pub(crate) fn with_model_turns(max_model_turns: u32) -> Result<Self, ManifestError> {
        if !(1..=MAX_HOST_MODEL_TURNS).contains(&max_model_turns) {
            return Err(ManifestError::Invalid);
        }
        Ok(Self {
            max_model_turns,
            max_model_calls: max_model_turns * 2,
            max_planned_steps: max_model_turns,
            max_tool_calls: max_model_turns,
            ..Self::workspace_discovery()
        })
    }

    pub(crate) fn agent_loop(&self) -> Result<AgentLoopBudget, ManifestError> {
        AgentLoopBudget::new(
            self.max_model_turns,
            self.max_planned_steps,
            self.max_tool_calls,
            self.max_context_bytes,
        )
        .map_err(|_| ManifestError::Invalid)
    }

    pub(crate) fn model_calls(&self) -> Result<ModelCallBudget, ManifestError> {
        ModelCallBudget::new(self.max_model_calls).map_err(|_| ManifestError::Invalid)
    }
}

impl RunManifest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        run_id: &str,
        workspace_id: &WorkspaceId,
        workspace_root_identity_profile: &str,
        workspace_root_identity_digest: &str,
        planner_id: &str,
        model: &str,
        tokenizer: &str,
        request_profile_digest: &str,
        allow_file_catalog_digest: &str,
        local_execution_profile_digest: &str,
        budget: ManifestBudget,
    ) -> Result<Self, ManifestError> {
        let record = RunManifestRecord {
            format_version: MANIFEST_FORMAT_VERSION,
            run_id: run_id.to_owned(),
            workspace_id: workspace_id.as_str().to_owned(),
            workspace_root_identity_profile: workspace_root_identity_profile.to_owned(),
            workspace_root_identity_digest: workspace_root_identity_digest.to_owned(),
            planner_id: planner_id.to_owned(),
            model: model.to_owned(),
            tokenizer: tokenizer.to_owned(),
            request_profile_digest: request_profile_digest.to_owned(),
            model_data_boundary: ModelDataBoundary::Remote,
            allow_file_catalog_digest: allow_file_catalog_digest.to_owned(),
            local_execution_profile_digest: local_execution_profile_digest.to_owned(),
            process_fingerprints: None,
            tool_discovery_snapshots: crate::graph_discovery::Snapshots::new(),
            budget,
            conversation_responses: false,
            final_response_schema: None,
            completion_checks: None,
        };
        validate_record(&record)?;
        let record_digest = digest_record(&record)?;
        Ok(Self {
            record,
            record_digest,
        })
    }

    pub(crate) fn with_tool_discovery_snapshots(
        mut self,
        snapshots: crate::graph_discovery::Snapshots,
    ) -> Result<Self, ManifestError> {
        self.record.tool_discovery_snapshots = snapshots;
        validate_record(&self.record)?;
        self.record_digest = digest_record(&self.record)?;
        Ok(self)
    }

    pub(crate) fn tool_discovery_snapshots(&self) -> &crate::graph_discovery::Snapshots {
        &self.record.tool_discovery_snapshots
    }

    pub(crate) fn with_final_response_schema(
        mut self,
        schema: Option<serde_json::Value>,
    ) -> Result<Self, ManifestError> {
        self.record.final_response_schema = schema.map(SavedFinalResponseSchema);
        validate_record(&self.record)?;
        self.record_digest = digest_record(&self.record)?;
        Ok(self)
    }

    pub(crate) fn with_completion_checks(
        mut self,
        checks: Option<Vec<xgen_provider_openai::CompletionCheck>>,
    ) -> Result<Self, ManifestError> {
        self.record.completion_checks = checks;
        validate_record(&self.record)?;
        self.record_digest = digest_record(&self.record)?;
        Ok(self)
    }

    pub(crate) fn completion_checks(&self) -> Option<&[xgen_provider_openai::CompletionCheck]> {
        self.record.completion_checks.as_deref()
    }

    pub(crate) fn final_response_schema(&self) -> Option<&serde_json::Value> {
        self.record
            .final_response_schema
            .as_ref()
            .map(|schema| &schema.0)
    }

    pub(crate) fn with_conversation_responses(mut self) -> Result<Self, ManifestError> {
        self.record.conversation_responses = true;
        self.record_digest = digest_record(&self.record)?;
        Ok(self)
    }

    pub(crate) fn with_process_fingerprints(
        mut self,
        fingerprints: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, ManifestError> {
        self.record.process_fingerprints = Some(fingerprints);
        validate_record(&self.record)?;
        self.record_digest = digest_record(&self.record)?;
        Ok(self)
    }

    pub(crate) fn process_fingerprints(
        &self,
    ) -> Option<&std::collections::BTreeMap<String, String>> {
        self.record.process_fingerprints.as_ref()
    }

    pub(crate) fn conversation_responses(&self) -> bool {
        self.record.conversation_responses
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::Invalid);
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| ManifestError::Invalid)?;
        manifest.verify()?;
        Ok(manifest)
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        self.verify()?;
        let bytes = serde_jcs::to_vec(self).map_err(|_| ManifestError::Canonicalization)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::Invalid);
        }
        Ok(bytes)
    }

    pub(crate) fn verify(&self) -> Result<(), ManifestError> {
        validate_record(&self.record)?;
        if self.record_digest != digest_record(&self.record)? {
            return Err(ManifestError::DigestMismatch);
        }
        Ok(())
    }

    pub(crate) fn authority(&self) -> String {
        let digest = self
            .record_digest
            .strip_prefix("sha256:")
            .expect("verified manifest digest is canonical");
        format!("{AUTHORITY_PREFIX}{digest}")
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.record.run_id
    }

    pub(crate) fn workspace_id(&self) -> Result<WorkspaceId, ManifestError> {
        WorkspaceId::new(&self.record.workspace_id).map_err(|_| ManifestError::Invalid)
    }

    pub(crate) fn workspace_identity_profile(&self) -> &str {
        &self.record.workspace_root_identity_profile
    }

    pub(crate) fn workspace_identity_digest(&self) -> &str {
        &self.record.workspace_root_identity_digest
    }

    pub(crate) fn planner_id(&self) -> &str {
        &self.record.planner_id
    }

    pub(crate) fn model(&self) -> &str {
        &self.record.model
    }

    pub(crate) fn tokenizer(&self) -> &str {
        &self.record.tokenizer
    }

    pub(crate) fn request_profile_digest(&self) -> &str {
        &self.record.request_profile_digest
    }

    pub(crate) fn allow_file_catalog_digest(&self) -> &str {
        &self.record.allow_file_catalog_digest
    }

    pub(crate) fn local_execution_profile_digest(&self) -> &str {
        &self.record.local_execution_profile_digest
    }

    pub(crate) const fn budget(&self) -> &ManifestBudget {
        &self.record.budget
    }

    pub(crate) const fn remote_model_egress(&self) -> bool {
        matches!(self.record.model_data_boundary, ModelDataBoundary::Remote)
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestError {
    #[error("run manifest is invalid")]
    Invalid,
    #[error("run manifest digest does not match")]
    DigestMismatch,
    #[error("run manifest could not be canonicalized")]
    Canonicalization,
}

fn validate_record(record: &RunManifestRecord) -> Result<(), ManifestError> {
    if let Some(schema) = &record.final_response_schema {
        let encoded = serde_jcs::to_string(schema).map_err(|_| ManifestError::Invalid)?;
        xgen_provider_openai::parse_completion_schema(&encoded)
            .map_err(|_| ManifestError::Invalid)?;
    }
    if let Some(checks) = &record.completion_checks {
        let schema = record
            .final_response_schema
            .as_ref()
            .ok_or(ManifestError::Invalid)?;
        let contract = serde_json::json!({"response_schema":schema.0,"checks":checks});
        xgen_provider_openai::parse_completion_contract(&contract.to_string())
            .map_err(|_| ManifestError::Invalid)?;
    }
    if record.process_fingerprints.as_ref().is_some_and(|items| {
        items.len() > 128
            || items.iter().any(|(key, value)| {
                key.len() > 256
                    || key.chars().any(char::is_control)
                    || !(key.starts_with("executable:") || key.starts_with("environment:"))
                    || !valid_sha256_digest(value)
            })
    }) {
        return Err(ManifestError::Invalid);
    }
    if !crate::graph_discovery::valid_snapshots(&record.tool_discovery_snapshots)
        || record.format_version != MANIFEST_FORMAT_VERSION
        || !valid_run_id(&record.run_id)
        || WorkspaceId::new(&record.workspace_id).is_err()
        || !valid_identifier(&record.workspace_root_identity_profile, 128)
        || !valid_sha256_digest(&record.workspace_root_identity_digest)
        || !valid_identifier(&record.planner_id, 256)
        || !valid_profile_text(&record.model)
        || !valid_profile_text(&record.tokenizer)
        || !valid_sha256_digest(&record.request_profile_digest)
        || !valid_sha256_digest(&record.allow_file_catalog_digest)
        || !valid_sha256_digest(&record.local_execution_profile_digest)
        || record.budget.agent_loop().is_err()
        || record.budget.model_calls().is_err()
        || record.budget.max_model_calls < record.budget.max_model_turns
    {
        return Err(ManifestError::Invalid);
    }
    Ok(())
}

pub(crate) fn valid_run_id(value: &str) -> bool {
    value.len() == 36
        && value.strip_prefix("run-").is_some_and(|encoded| {
            encoded.len() == 32
                && encoded
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

fn valid_identifier(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_profile_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROFILE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|encoded| {
        encoded.len() == 64
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn digest_record(record: &RunManifestRecord) -> Result<String, ManifestError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DigestInput<'a> {
        domain: &'static str,
        record: &'a RunManifestRecord,
    }

    let canonical = serde_jcs::to_vec(&DigestInput {
        domain: "xgeny.cli.run-manifest/v1",
        record,
    })
    .map_err(|_| ManifestError::Canonicalization)?;
    Ok(format!("sha256:{}", sha256_hex(&canonical)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const DIGEST_A: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIGEST_C: &str =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DIGEST_D: &str =
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    fn fixture() -> RunManifest {
        RunManifest::new(
            "run-0123456789abcdef0123456789abcdef",
            &WorkspaceId::new("ws-0123456789abcdef").unwrap(),
            "xgeny.workspace-root.unix-file-id.v1",
            DIGEST_A,
            "xgeny.cli.openai",
            "qwen3.8-27b",
            "Qwen-Qwen3.8-27B-FP8",
            DIGEST_B,
            DIGEST_C,
            DIGEST_D,
            ManifestBudget::default(),
        )
        .expect("manifest should construct")
    }

    #[test]
    fn discovery_snapshots_are_durable_and_manifest_bound() {
        let legacy = fixture();
        assert!(legacy.tool_discovery_snapshots().is_empty());
        assert!(
            !String::from_utf8(legacy.to_bytes().unwrap())
                .unwrap()
                .contains("toolDiscoverySnapshots")
        );
        let bound = legacy
            .clone()
            .with_tool_discovery_snapshots(crate::graph_discovery::Snapshots::from([
                ("assets".into(), "a".repeat(64)),
                ("calendar".into(), "b".repeat(64)),
            ]))
            .unwrap();
        assert_ne!(legacy.authority(), bound.authority());
        assert_eq!(
            RunManifest::from_bytes(&bound.to_bytes().unwrap()).unwrap(),
            bound
        );
        for collection in ["assets", "calendar"] {
            let mut tampered: Value = serde_json::from_slice(&bound.to_bytes().unwrap()).unwrap();
            tampered["record"]["toolDiscoverySnapshots"][collection] =
                Value::String("c".repeat(64));
            assert!(RunManifest::from_bytes(&serde_json::to_vec(&tampered).unwrap()).is_err());
        }
    }

    #[test]
    fn optional_diagnostics_keep_legacy_manifests_and_bind_new_hashes() {
        let legacy = fixture();
        assert!(legacy.process_fingerprints().is_none());
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert("environment:LANG".to_owned(), DIGEST_A.to_owned());
        let manifest = legacy.with_process_fingerprints(hashes).unwrap();
        assert_eq!(
            RunManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap(),
            manifest
        );
        let mut value: Value = serde_json::from_slice(&manifest.to_bytes().unwrap()).unwrap();
        value["record"]["processFingerprints"]["environment:LANG"] =
            Value::String(DIGEST_B.to_owned());
        assert!(RunManifest::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        let invalid = std::collections::BTreeMap::from([(
            "environment:LANG".to_owned(),
            "plaintext".to_owned(),
        )]);
        assert!(fixture().with_process_fingerprints(invalid).is_err());
    }

    #[test]
    fn canonical_round_trip_binds_authority() {
        let manifest = fixture();
        let bytes = manifest.to_bytes().expect("manifest should serialize");
        let loaded = RunManifest::from_bytes(&bytes).expect("manifest should load");
        assert_eq!(loaded, manifest);
        assert!(loaded.authority().starts_with(AUTHORITY_PREFIX));
        assert_eq!(loaded.authority().len(), AUTHORITY_PREFIX.len() + 64);
    }

    #[test]
    fn final_response_schema_is_bound_redacted_and_omitted_for_legacy_records() {
        let original = fixture();
        assert!(
            !String::from_utf8(original.to_bytes().unwrap())
                .unwrap()
                .contains("finalResponseSchema")
        );
        let schema = serde_json::json!({"type":"object","properties":{"label":{"type":"string","enum":["SCHEMA-SENTINEL"]}},"required":["label"],"additionalProperties":false});
        let bound = original
            .clone()
            .with_final_response_schema(Some(schema.clone()))
            .unwrap();
        assert_ne!(bound.authority(), original.authority());
        assert!(!format!("{bound:?}").contains("SCHEMA-SENTINEL"));
        assert_eq!(
            RunManifest::from_bytes(&bound.to_bytes().unwrap())
                .unwrap()
                .final_response_schema(),
            Some(&schema)
        );
        assert!(
            original
                .with_final_response_schema(Some(serde_json::json!({"type":"array"})))
                .is_err()
        );
    }

    #[test]
    fn completion_checks_are_manifest_bound_and_require_a_saved_schema() {
        use xgen_provider_openai::CompletionCheck;
        let check = CompletionCheck {
            id: "result".into(),
            argv: vec!["verify".into()],
        };
        assert!(
            fixture()
                .with_completion_checks(Some(vec![check.clone()]))
                .is_err()
        );
        let base = fixture()
            .with_final_response_schema(Some(serde_json::json!({"type":"object"})))
            .unwrap();
        assert!(
            !String::from_utf8(base.to_bytes().unwrap())
                .unwrap()
                .contains("completionChecks")
        );
        let bound = base
            .clone()
            .with_completion_checks(Some(vec![check.clone()]))
            .unwrap();
        assert_ne!(base.authority(), bound.authority());
        assert_eq!(
            RunManifest::from_bytes(&bound.to_bytes().unwrap())
                .unwrap()
                .completion_checks(),
            Some([check].as_slice())
        );
        assert!(base.with_completion_checks(Some(Vec::new())).is_err());
    }

    #[test]
    fn tampering_and_unknown_fields_fail_closed() {
        let manifest = fixture();
        let mut value: Value = serde_json::from_slice(&manifest.to_bytes().unwrap()).unwrap();
        value["record"]["model"] = Value::String("changed-model".to_owned());
        assert_eq!(
            RunManifest::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
            ManifestError::DigestMismatch
        );
        value["unknown"] = Value::Bool(true);
        assert_eq!(
            RunManifest::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
            ManifestError::Invalid
        );
    }

    #[test]
    fn host_model_turn_budget_keeps_discovery_ratio_and_round_trips() {
        let budget = ManifestBudget::with_model_turns(96).unwrap();
        assert_eq!(
            (
                budget.max_model_turns,
                budget.max_model_calls,
                budget.max_planned_steps,
                budget.max_tool_calls,
                budget.max_context_bytes,
            ),
            (
                96,
                192,
                96,
                96,
                ManifestBudget::workspace_discovery().max_context_bytes
            )
        );
        for turns in [0, MAX_HOST_MODEL_TURNS + 1] {
            assert_eq!(
                ManifestBudget::with_model_turns(turns).unwrap_err(),
                ManifestError::Invalid
            );
        }
        let largest = ManifestBudget::with_model_turns(MAX_HOST_MODEL_TURNS).unwrap();
        assert!(largest.agent_loop().is_ok() && largest.model_calls().is_ok());
        let manifest = RunManifest::new(
            "run-0123456789abcdef0123456789abcdef",
            &WorkspaceId::new("ws-0123456789abcdef").unwrap(),
            "xgeny.workspace-root.unix-file-id.v1",
            DIGEST_A,
            "xgeny.cli.openai",
            "qwen3.8-27b",
            "Qwen-Qwen3.8-27B-FP8",
            DIGEST_B,
            DIGEST_C,
            DIGEST_D,
            budget.clone(),
        )
        .unwrap();
        let loaded = RunManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
        assert_eq!(loaded.budget(), &budget);
        assert_ne!(loaded.authority(), fixture().authority());
    }

    #[test]
    fn manifest_excludes_endpoint_paths_goal_and_credentials() {
        let text = String::from_utf8(fixture().to_bytes().unwrap()).unwrap();
        for forbidden in [
            "http://127.0.0.1:18000",
            "/home/user/workspace",
            "README.md",
            "read the secret file",
            "Bearer",
        ] {
            assert!(!text.contains(forbidden));
        }
        assert!(text.contains("remote"));
    }
}
