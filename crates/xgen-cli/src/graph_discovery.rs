//! Read-only discovery capabilities pinned by the host; candidates are never executable.
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use xgen_domain::{CapabilityRef, InstanceBinding, VerificationResult, VerificationStrategy};
use xgen_policy::ResourceResolutionFailure;
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure,
    AdapterPrepareRequest, AdapterReconcileRequest, AdapterReconciliationInconclusiveReason,
    AdapterReconciliationObservation, AdapterToolOutput, EffectAdapter, EffectVerifier,
    PreparedAdapterInvocation, RuleVerificationObservation, VerificationPortFailure,
    VerificationReport, VerificationRequest, VerifiedArtifactDescriptor, VerifierOutputDigest,
};
use xgen_workgraph::EffectClass;

pub(crate) const SEARCH: &str = "xgen.tools/search";
pub(crate) const DESCRIBE: &str = "xgen.tools/describe";
pub(crate) const VERSION: &str = "1.0.0";
pub(crate) fn version(operation: &str) -> &'static str {
    if operation == DESCRIBE {
        "2.0.0"
    } else {
        VERSION
    }
}
pub(crate) const SCOPE: &str = "tools.discover";
pub(crate) const PROVIDER: &str = "xgen.cli.tool-discovery-material.v1";
pub(crate) const MAX_OUTPUT: usize = 64 * 1024;
pub(crate) type Snapshots = BTreeMap<String, String>;
/// Leading characters of the run goal sent for script detection. The decision is made from
/// the goal's letters, so a bounded prefix keeps the worker request under its size limit.
const LANGUAGE_REQUEST_CHARS: usize = 4096;
const BILINGUAL_QUERY_ONE: &str = "The request is written in a different script from this collection's documentation: write the xgen.tools/search query with the request's key terms in both the request's language and the documentation's language. ";
const BILINGUAL_QUERY_MANY: &str = "The request is written in a different script from the documentation of each collection marked with a documentation script: for those collections, write the xgen.tools/search query with the request's key terms in both the request's language and the documentation's language. ";

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub(crate) fn valid_snapshots(items: &Snapshots) -> bool {
    items.len() <= 16
        && items.iter().all(|(name, digest)| {
            valid_name(name)
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
}

pub(crate) fn resolve(resource: &str) -> Result<String, ResourceResolutionFailure> {
    valid_name(resource)
        .then(|| resource.to_owned())
        .ok_or(ResourceResolutionFailure::InvalidResource)
}

#[derive(Clone)]
pub(crate) struct DiscoveryCatalog {
    root: PathBuf,
    snapshots: Snapshots,
    connections: crate::http_read::Connections,
    /// Documentation script of each collection whose script differs from the run goal's.
    language: BTreeMap<String, String>,
}

impl DiscoveryCatalog {
    /// Setup is host-side. Per-step retrieval uses offline mode and cannot install packages.
    pub(crate) fn discover() -> Result<Snapshots, ()> {
        let root = crate::tool_catalog_directory().map_err(|_| ())?;
        if !root.exists() {
            return Ok(Snapshots::new());
        }
        let count = std::fs::read_dir(&root)
            .map_err(|_| ())?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json.gz"))
            .count();
        if count == 0 {
            return Ok(Snapshots::new());
        }
        if count > 16 {
            return Err(());
        }
        let response =
            crate::tools::invoke(&json!({"operation":"list", "root":root.to_str().ok_or(())?}))
                .map_err(|_| ())?;
        if response["ok"] != true {
            return Err(());
        }
        let mut snapshots = Snapshots::new();
        for item in response["collections"].as_array().ok_or(())? {
            if item["backend_version"] != "0.47.0" || item["execution_enabled"] != false {
                return Err(());
            }
            let name = item["collection"].as_str().ok_or(())?.to_owned();
            let digest = item["artifact_digest"].as_str().ok_or(())?.to_owned();
            if snapshots.insert(name, digest).is_some() {
                return Err(());
            }
        }
        valid_snapshots(&snapshots).then_some(snapshots).ok_or(())
    }

    pub(crate) fn saved(snapshots: Snapshots) -> Result<Self, ()> {
        if !valid_snapshots(&snapshots) {
            return Err(());
        }
        Ok(Self {
            root: crate::tool_catalog_directory().map_err(|_| ())?,
            snapshots,
            connections: crate::http_read::Connections::new(),
            language: BTreeMap::new(),
        })
    }

    /// Mark collections documented in a different script from the run goal. The worker
    /// derives both scripts from the goal and the pinned artifacts, so a resumed run gets
    /// the same planner hint. The hint is advisory: when it cannot be derived (for example
    /// a changed snapshot, which search reports as a failed observation), the unchanged
    /// hint is used.
    pub(crate) fn with_request_language(mut self, goal: &str) -> Self {
        if self.snapshots.is_empty() {
            return self;
        }
        match self.request_language(goal) {
            Ok(language) => self.language = language,
            Err(()) => eprintln!("XGEN_TOOLS warning=language_hint_unavailable"),
        }
        self
    }

    fn request_language(&self, goal: &str) -> Result<BTreeMap<String, String>, ()> {
        let request = goal
            .chars()
            .take(LANGUAGE_REQUEST_CHARS)
            .collect::<String>();
        let output = crate::tools::invoke_offline(
            &json!({"operation":"language_hints","root":self.root.to_str().ok_or(())?,"request":request,"snapshots":self.snapshots}),
        )
        .map_err(|_| ())?;
        let rows = output["collections"]
            .as_object()
            .filter(|rows| output["ok"] == true && rows.len() == self.snapshots.len())
            .ok_or(())?;
        let mut language = BTreeMap::new();
        for name in self.snapshots.keys() {
            let row = rows.get(name).ok_or(())?;
            let script = &row["documentation_script"];
            if !script.is_null()
                && !script.as_str().is_some_and(|s| {
                    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic())
                })
            {
                return Err(());
            }
            if row["script_differs"].as_bool().ok_or(())? {
                language.insert(name.clone(), script.as_str().ok_or(())?.to_owned());
            }
        }
        Ok(language)
    }

    pub(crate) fn with_connections(
        mut self,
        connections: crate::http_read::Connections,
    ) -> Result<Self, ()> {
        if !crate::http_read::valid_connections(&connections)
            || connections.keys().any(|name| !self.contains(name))
        {
            return Err(());
        }
        self.connections = connections;
        Ok(self)
    }
    pub(crate) fn connection(&self, name: &str) -> Option<&crate::http_read::Connection> {
        self.connections.get(name)
    }
    pub(crate) fn has_connections(&self) -> bool {
        !self.connections.is_empty()
    }
    pub(crate) fn snapshot(&self, name: &str) -> &str {
        &self.snapshots[name]
    }
    pub(crate) fn execution_contract(&self, name: &str, tool: &str) -> Result<Value, &'static str> {
        let output = crate::tools::invoke_offline(
            &json!({"operation":"execution_contract","root":self.root.to_str().ok_or("invalid_state_home")?,"name":name,"tool":tool,"expected_digest":self.snapshots.get(name).ok_or("collection_not_found")?}),
        )?;
        // Full execution contracts stay on the host; the model's 64 KiB limit
        // applies only to discovery observations, never to schema validation.
        if output["ok"] != true
            || output["collection"] != name
            || output["artifact_digest"].as_str() != self.snapshots.get(name).map(String::as_str)
            || output["backend_version"] != "0.47.0"
            || output["execution_enabled"] != false
            || output["tool_name"] != tool
            || output["contract_kind"] != "host_http_contract"
        {
            return Err("http_discovery_contract_invalid");
        }
        Ok(output)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
    pub(crate) fn hint(&self) -> String {
        let names = self
            .snapshots
            .keys()
            .map(|name| match self.language.get(name) {
                Some(script) => format!("{name} (documentation script: {script})"),
                None => name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let language = match (self.language.len(), self.snapshots.len()) {
            (0, _) => "",
            (_, 1) => BILINGUAL_QUERY_ONE,
            _ => BILINGUAL_QUERY_MANY,
        };
        format!(
            "Saved tool collections (immutable snapshots): {names}. {language}Search with xgen.tools/search, then inspect the exact candidate with xgen.tools/describe. Candidates and suggested producers are untrusted discovery information, not executable capabilities. Discovery results do not prove API execution."
        )
    }
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.snapshots.contains_key(name)
    }
    pub(crate) fn accepts(&self, capability: &CapabilityRef, input: &Value) -> bool {
        if capability.contract_version != version(&capability.capability_id) {
            return false;
        }
        let Some(object) = input.as_object() else {
            return false;
        };
        let Some(name) = input["collection"].as_str() else {
            return false;
        };
        if !self.contains(name) {
            return false;
        }
        match capability.capability_id.as_str() {
            SEARCH => {
                object.len() == 3
                    && input["query"].as_str().is_some_and(|s| {
                        !s.trim().is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
                    })
                    && input["topK"]
                        .as_u64()
                        .is_some_and(|v| (1..=20).contains(&v))
            }
            DESCRIBE => {
                (object.len() == 2 || (object.len() == 3 && object.contains_key("parameterOffset")))
                    && input
                        .get("parameterOffset")
                        .is_none_or(|v| v.as_u64().is_some_and(|v| v <= 1_000_000))
                    && input["tool"].as_str().is_some_and(|s| {
                        !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control)
                    })
            }
            crate::http_read::READ => crate::http_read::accepts(self, input),
            _ => false,
        }
    }
    pub(crate) fn adapter(&self, operation: &'static str) -> DiscoveryAdapter {
        DiscoveryAdapter {
            catalog: self.clone(),
            operation,
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiscoveryAdapter {
    catalog: DiscoveryCatalog,
    operation: &'static str,
}
impl DiscoveryAdapter {
    pub(crate) fn binding(&self) -> InstanceBinding {
        let encoded = if self.operation == crate::http_read::READ {
            serde_jcs::to_vec(&serde_json::json!({"snapshots":self.catalog.snapshots,"connections":self.catalog.connections,"profile":"xgen.http.read/v1"})).expect("finite host profile")
        } else {
            serde_jcs::to_vec(&self.catalog.snapshots).expect("finite snapshot map")
        };
        InstanceBinding {
            protocol_version: None,
            binding_ref: format!(
                "builtin://tool-discovery/{}",
                digest(&encoded).trim_start_matches("sha256:")
            ),
            operation_ref: Some(format!(
                "{}@{};graph-tool-call=0.47.0;offline",
                self.operation,
                version(self.operation)
            )),
        }
    }
    pub(crate) fn verifier(&self) -> DiscoveryVerifier {
        DiscoveryVerifier {
            adapter: self.clone(),
        }
    }
    fn request(&self, input: &Value) -> Result<Value, AdapterPrepareFailure> {
        if !self.catalog.accepts(
            &CapabilityRef {
                capability_id: self.operation.into(),
                contract_version: version(self.operation).into(),
            },
            input,
        ) {
            return Err(AdapterPrepareFailure::InvalidMaterial);
        }
        let name = input["collection"]
            .as_str()
            .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
        let mut request = json!({"operation":if self.operation == SEARCH {"search"} else {"describe_view"}, "root":self.catalog.root.to_str().ok_or(AdapterPrepareFailure::ResourceUnavailable)?, "name":name,"expected_digest":self.catalog.snapshots[name]});
        if self.operation == SEARCH {
            request["query"] = input["query"].clone();
            request["top_k"] = input["topK"].clone();
        } else {
            request["tool"] = input["tool"].clone();
            request["parameter_offset"] = input.get("parameterOffset").cloned().unwrap_or(json!(0));
        }
        Ok(request)
    }
}

impl EffectAdapter for DiscoveryAdapter {
    fn prepare(
        &mut self,
        request: AdapterPrepareRequest<'_>,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        let intent = request.intent();
        let instance = request.instance();
        if intent.invocation.capability_id != self.operation
            || intent.invocation.contract_version != version(self.operation)
            || instance.definition.capability_id != self.operation
            || instance.definition.contract_version != version(self.operation)
            || intent.invocation.instance_id != instance.instance_id
            || instance.binding != self.binding()
            || intent.effect_class != EffectClass::ReadOnly
            || intent.idempotency_key.is_some()
            || !instance.features.sync
            || instance.features.task
            || instance.features.cancellable
            || instance.features.idempotency_query
        {
            return Err(AdapterPrepareFailure::UnsupportedProtocol);
        }
        if self.operation == crate::http_read::READ {
            return crate::http_read::prepare(&self.catalog, request.normalized_arguments());
        }
        Ok(Box::new(PreparedDiscovery {
            adapter: self.clone(),
            input: request.normalized_arguments().clone(),
            request: self.request(request.normalized_arguments())?,
        }))
    }
    fn reconcile(&mut self, _: AdapterReconcileRequest<'_>) -> AdapterReconciliationObservation {
        AdapterReconciliationObservation::Inconclusive {
            reason: AdapterReconciliationInconclusiveReason::StableKeyUnsupported,
        }
    }
}

struct PreparedDiscovery {
    adapter: DiscoveryAdapter,
    input: Value,
    request: Value,
}
impl PreparedAdapterInvocation for PreparedDiscovery {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        let result = crate::tools::invoke_offline(&self.request)
            .map_err(str::to_owned)
            .and_then(|output| {
                if output["ok"] == false {
                    return Err(output["error"]
                        .as_str()
                        .filter(|s| valid_error(s))
                        .unwrap_or("tool_discovery_backend_failed")
                        .to_owned());
                }
                if serde_jcs::to_vec(&output)
                    .map_err(|_| "tool_discovery_response_invalid".to_owned())?
                    .len()
                    > MAX_OUTPUT
                {
                    return Err("tool_discovery_output_limit".to_owned());
                }
                inspect(
                    &self.adapter.catalog,
                    self.adapter.operation,
                    &self.input,
                    &output,
                )
                .map_err(|()| "tool_discovery_response_invalid".to_owned())?;
                Ok(output)
            });
        let mut output = result.unwrap_or_else(|code| {
            json!({
                "ok":false, "error":code, "collection":self.input["collection"],
                "artifact_digest":self.request["expected_digest"], "backend_version":"0.47.0",
                "execution_enabled":false, "snapshot_verified":false,
            })
        });
        output["request"] = self.input.clone();
        if serde_jcs::to_vec(&output)
            .ok()
            .is_none_or(|bytes| bytes.len() > MAX_OUTPUT)
        {
            output = json!({"ok":false,"error":"tool_discovery_output_limit","collection":self.input["collection"],"artifact_digest":self.request["expected_digest"],"backend_version":"0.47.0","execution_enabled":false,"snapshot_verified":false,"request":self.input});
        }
        let bytes = serde_jcs::to_vec(&output).expect("bounded finite discovery output");
        AdapterExecutionObservation::SucceededWithOutput {
            evidence_digest: AdapterEvidenceDigest::new(digest(&bytes)).expect("canonical digest"),
            output: AdapterToolOutput::new(output),
        }
    }
}

fn hex_digest(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_error(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 80
        && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn inspect(
    catalog: &DiscoveryCatalog,
    operation: &str,
    input: &Value,
    output: &Value,
) -> Result<(), ()> {
    let bytes = serde_jcs::to_vec(output).map_err(|_| ())?;
    let name = input["collection"].as_str().ok_or(())?;
    if bytes.len() > MAX_OUTPUT
        || !output["ok"].is_boolean()
        || output["collection"] != name
        || output["artifact_digest"].as_str() != catalog.snapshots.get(name).map(String::as_str)
        || output["backend_version"] != "0.47.0"
        || output["execution_enabled"] != (operation == crate::http_read::READ)
    {
        return Err(());
    }
    if operation == crate::http_read::READ {
        return crate::http_read::inspect(catalog, input, output);
    }
    if output["ok"] == false {
        return if output["snapshot_verified"] == false
            && output["error"].as_str().is_some_and(|s| {
                !s.is_empty()
                    && s.len() <= 80
                    && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
            && output
                .as_object()
                .is_some_and(|v| v.len() == 8 || (v.len() == 7 && !v.contains_key("request")))
        {
            Ok(())
        } else {
            Err(())
        };
    }
    if operation == SEARCH {
        let rows = output["candidates"].as_array().ok_or(())?;
        if rows.len() > usize::try_from(input["topK"].as_u64().ok_or(())?).map_err(|_| ())?
            || output["relations_verified_by_execution"] != false
            || output["possible_producers"]
                .as_array()
                .is_none_or(|v| v.len() > 40 || v.iter().any(|item| item.as_str().is_none()))
            || rows.iter().any(|row| {
                row["tool"].as_str().is_none_or(str::is_empty)
                    || row["description"].as_str().is_none()
                    || row["score"].as_f64().is_none()
            })
        {
            return Err(());
        }
    } else if operation == DESCRIBE {
        let page = &output["parameter_page"];
        let offset = input
            .get("parameterOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = page["total"].as_u64().ok_or(())?;
        let rows = output["tool"]["parameters"].as_array().ok_or(())?;
        let end = offset.checked_add(rows.len() as u64).ok_or(())?;
        if output["contract_kind"] != "discovery_view"
            || output["view_version"] != 2
            || output["complete"] != false
            || output["effect_class"] != "unclassified"
            || output["tool"]["name"] != input["tool"]
            || !output["tool"]["metadata"]["api_contract"].is_object()
            || !hex_digest(&output["full_tool_digest"])
            || page["offset"] != offset
            || rows.len() > 32
            || end > total
            || (end < total && (rows.is_empty() || page["next_offset"] != end))
            || (end == total && !page["next_offset"].is_null())
            || rows.iter().enumerate().any(|(i, row)| {
                row["index"] != offset + i as u64
                    || !row["complete"].is_boolean()
                    || !hex_digest(&row["schema_digest"])
            })
            || !output["http_read"]["supported"].is_boolean()
            || (output["http_read"]["supported"] == true
                && (!hex_digest(&output["http_read"]["contract_digest"])
                    || output["http_read"].get("contract").is_some()))
        {
            return Err(());
        }
    } else {
        return Err(());
    }
    Ok(())
}

fn verified_request<'a>(
    catalog: &DiscoveryCatalog,
    capability: &CapabilityRef,
    output: &'a Value,
    material_digest: &str,
) -> Result<&'a Value, VerificationPortFailure> {
    let input = &output["request"];
    if !catalog.accepts(capability, input)
        || xgen_workgraph::invocation_material_digest(input)
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?
            != material_digest
    {
        return Err(VerificationPortFailure::ResponseUnverifiable);
    }
    Ok(input)
}

pub(crate) struct DiscoveryVerifier {
    adapter: DiscoveryAdapter,
}
impl EffectVerifier for DiscoveryVerifier {
    fn verify(
        &mut self,
        request: VerificationRequest<'_>,
    ) -> Result<VerificationReport, VerificationPortFailure> {
        let instance = request.instance();
        let intent = request.intent();
        let rules = &request.definition().spec.verification;
        if instance.binding != self.adapter.binding()
            || intent.invocation.capability_id != self.adapter.operation
            || intent.invocation.contract_version != version(self.adapter.operation)
            || intent.invocation.instance_id != instance.instance_id
            || intent.effect_class != EffectClass::ReadOnly
            || intent.idempotency_key.is_some()
            || rules.len() != 2
            || rules.iter().any(|r| {
                !r.required
                    || !matches!(
                        r.strategy,
                        VerificationStrategy::OutputSchema | VerificationStrategy::Postcondition
                    )
            })
            || rules
                .iter()
                .filter(|r| r.strategy == VerificationStrategy::OutputSchema)
                .count()
                != 1
        {
            return Err(VerificationPortFailure::UnsupportedStrategy);
        }
        let output = request
            .tool_output()
            .ok_or(VerificationPortFailure::EvidenceUnavailable)?;
        let input = verified_request(
            &self.adapter.catalog,
            &instance.definition,
            output.output(),
            &intent.authorization.binding.material_digest,
        )?;
        inspect(
            &self.adapter.catalog,
            self.adapter.operation,
            input,
            output.output(),
        )
        .map_err(|()| VerificationPortFailure::ResponseUnverifiable)?;
        let bytes = serde_jcs::to_vec(output.output())
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        let evidence = digest(&bytes);
        if evidence != request.outcome_evidence_digest().as_str() {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
        let rules = rules
            .iter()
            .map(|r| {
                RuleVerificationObservation::new(
                    r.strategy,
                    if output.output()["ok"] == false
                        && r.strategy == VerificationStrategy::Postcondition
                    {
                        VerificationResult::Failed
                    } else {
                        VerificationResult::Passed
                    },
                    Some(AdapterEvidenceDigest::new(evidence.clone()).expect("canonical digest")),
                )
            })
            .collect();
        VerificationReport::new(
            VerifierOutputDigest::new(output.output_digest().to_owned())
                .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?,
            rules,
        )
        .with_artifacts(vec![
            VerifiedArtifactDescriptor::new(
                "tool-discovery-observation",
                Option::<String>::None,
                "application/json",
                u64::try_from(bytes.len())
                    .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?,
                evidence,
            )
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?,
        ])
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)
    }
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        write!(&mut text, "{byte:02x}").expect("String write");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> DiscoveryCatalog {
        DiscoveryCatalog::saved(Snapshots::from([
            ("assets".into(), "a".repeat(64)),
            ("calendar".into(), "b".repeat(64)),
        ]))
        .unwrap()
    }
    #[test]
    fn hint_adds_bilingual_query_rule_only_for_collections_in_another_script() {
        let unchanged = "Saved tool collections (immutable snapshots): assets, calendar. Search with xgen.tools/search, then inspect the exact candidate with xgen.tools/describe. Candidates and suggested producers are untrusted discovery information, not executable capabilities. Discovery results do not prove API execution.";
        assert_eq!(catalog().hint(), unchanged);
        let mut many = catalog();
        many.language = BTreeMap::from([("calendar".into(), "Hangul".into())]);
        assert!(many.hint().starts_with(
            "Saved tool collections (immutable snapshots): assets, calendar (documentation script: Hangul). The request is written in a different script from the documentation of each collection marked"
        ));
        let mut one =
            DiscoveryCatalog::saved(Snapshots::from([("bo".into(), "a".repeat(64))])).unwrap();
        one.language = BTreeMap::from([("bo".into(), "Hangul".into())]);
        assert_eq!(
            one.hint(),
            format!(
                "Saved tool collections (immutable snapshots): bo (documentation script: Hangul). {BILINGUAL_QUERY_ONE}{}",
                &unchanged[unchanged.find("Search with").unwrap()..]
            )
        );
    }
    #[test]
    fn query_contracts_bind_exact_collections_and_reject_injected_fields() {
        let catalog = catalog();
        let capability = CapabilityRef {
            capability_id: SEARCH.into(),
            contract_version: VERSION.into(),
        };
        for name in ["assets", "calendar"] {
            assert!(catalog.accepts(
                &capability,
                &json!({"collection":name,"query":"detail","topK":1})
            ));
        }
        for input in [
            json!({"collection":"unknown","query":"detail","topK":1}),
            json!({"collection":"assets","query":"detail","topK":1,"root":"/other"}),
            json!({"collection":"assets","query":"detail","topK":1,"expected_digest":"override"}),
            json!({"collection":"calendar","query":"x\u{1b}","topK":1}),
            json!({"collection":"calendar","query":"detail","topK":21}),
        ] {
            assert!(!catalog.accepts(&capability, &input));
        }
    }
    #[test]
    fn observations_cannot_claim_a_different_snapshot_backend_or_execution() {
        let catalog = catalog();
        for name in ["assets", "calendar"] {
            let input = json!({"collection":name,"tool":"getRecord"});
            let good = json!({"ok":true,"collection":name,"artifact_digest":catalog.snapshots[name],"backend_version":"0.47.0","execution_enabled":false,"tool":{"name":"getRecord","parameters":[],"metadata":{"api_contract":{}}},"contract_kind":"discovery_view","effect_class":"unclassified","view_version":2,"complete":false,"full_tool_digest":"a".repeat(64),"parameter_page":{"offset":0,"total":0,"next_offset":null},"http_read":{"supported":false,"error":"http_original_source_unavailable"}});
            assert!(inspect(&catalog, DESCRIBE, &input, &good).is_ok());
            for (key, value) in [
                ("artifact_digest", json!("c".repeat(64))),
                ("collection", json!("other")),
                ("backend_version", json!("latest")),
                ("execution_enabled", json!(true)),
                ("effect_class", json!("read_only")),
            ] {
                let mut bad = good.clone();
                bad[key] = value;
                assert!(inspect(&catalog, DESCRIBE, &input, &bad).is_err());
            }
            let mut bad = good;
            bad["tool"]["name"] = json!("other");
            assert!(inspect(&catalog, DESCRIBE, &input, &bad).is_err());
        }
    }
    #[test]
    fn describe_requires_version_two_and_binds_parameter_pages() {
        let catalog = catalog();
        let current = CapabilityRef {
            capability_id: DESCRIBE.into(),
            contract_version: version(DESCRIBE).into(),
        };
        let old = CapabilityRef {
            contract_version: VERSION.into(),
            ..current.clone()
        };
        let page = json!({"collection":"assets","tool":"getRecord","parameterOffset":32});
        assert!(catalog.accepts(&current, &page));
        assert!(!catalog.accepts(&old, &page));
        for offset in [json!(-1), json!(true), json!(1_000_001), json!("32")] {
            let mut invalid = page.clone();
            invalid["parameterOffset"] = offset;
            assert!(!catalog.accepts(&current, &invalid));
        }
        let material = xgen_workgraph::invocation_material_digest(&page).unwrap();
        let mut altered = page.clone();
        altered["parameterOffset"] = json!(64);
        assert!(
            verified_request(&catalog, &current, &json!({"request":altered}), &material).is_err()
        );
        assert!(
            catalog
                .adapter(DESCRIBE)
                .binding()
                .operation_ref
                .unwrap()
                .contains("@2.0.0;")
        );
    }

    #[test]
    fn verification_uses_original_arguments_instead_of_self_reported_targets() {
        let catalog = catalog();
        for (id, original, altered) in [
            (
                SEARCH,
                json!({"collection":"assets","query":"details","topK":1}),
                json!({"collection":"assets","query":"details","topK":20}),
            ),
            (
                DESCRIBE,
                json!({"collection":"calendar","tool":"getRecord"}),
                json!({"collection":"calendar","tool":"otherRecord"}),
            ),
        ] {
            let capability = CapabilityRef {
                capability_id: id.into(),
                contract_version: version(id).into(),
            };
            let digest = xgen_workgraph::invocation_material_digest(&original).unwrap();
            assert!(
                verified_request(&catalog, &capability, &json!({"request":original}), &digest)
                    .is_ok()
            );
            assert!(
                verified_request(&catalog, &capability, &json!({"request":altered}), &digest)
                    .is_err()
            );
        }
    }

    #[test]
    fn snapshot_changes_alter_the_executable_binding() {
        let first = catalog();
        let mut second = first.clone();
        second.snapshots.insert("assets".into(), "c".repeat(64));
        assert_ne!(
            first.adapter(SEARCH).binding(),
            second.adapter(SEARCH).binding()
        );
        assert_ne!(
            first.adapter(SEARCH).binding(),
            first.adapter(DESCRIBE).binding()
        );
        assert!(!valid_snapshots(&Snapshots::from([(
            "../escape".into(),
            "a".repeat(64)
        )])));
    }
}
