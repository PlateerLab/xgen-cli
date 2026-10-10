//! Native run-scoped PTY operations; session loss never restarts a command.
#[cfg(target_os = "linux")]
pub(crate) mod sessions;

use crate::{ProcessExecuteVerifier, ProcessWorkspace};
use serde_json::{Value, json};
use xgen_domain::InstanceBinding;
use xgen_policy::ResourceResolutionFailure;
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure,
    AdapterPrepareRequest, AdapterReconcileRequest, AdapterReconciliationInconclusiveReason,
    AdapterReconciliationObservation, AdapterToolOutput, EffectAdapter, PreparedAdapterInvocation,
};

pub const TERMINAL_VERSION: &str = "1.0.0";
pub const TERMINAL_SCOPE: &str = "terminal.session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOperation {
    Start,
    Read,
    Write,
    Terminate,
}
impl TerminalOperation {
    pub const ALL: [Self; 4] = [Self::Start, Self::Read, Self::Write, Self::Terminate];
    #[must_use]
    pub const fn capability_id(self) -> &'static str {
        match self {
            Self::Start => "xgen.terminal/start",
            Self::Read => "xgen.terminal/read",
            Self::Write => "xgen.terminal/write",
            Self::Terminate => "xgen.terminal/terminate",
        }
    }
    #[must_use]
    pub fn from_capability(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.capability_id() == id)
    }
}

#[must_use]
pub const fn terminal_supported() -> bool {
    cfg!(target_os = "linux")
}

/// Preserve one opaque session handle without exposing a PID or ambient path.
/// # Errors
/// Rejects malformed handles.
pub fn resolve_terminal_session(resource: &str) -> Result<String, ResourceResolutionFailure> {
    let valid = resource.strip_prefix("pty-").is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    valid
        .then(|| resource.to_owned())
        .ok_or(ResourceResolutionFailure::InvalidResource)
}

#[derive(Clone)]
pub struct TerminalAdapter {
    workspace: ProcessWorkspace,
    operation: TerminalOperation,
}
impl std::fmt::Debug for TerminalAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalAdapter")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}
impl TerminalAdapter {
    pub(crate) const fn new(workspace: ProcessWorkspace, operation: TerminalOperation) -> Self {
        Self {
            workspace,
            operation,
        }
    }
    #[must_use]
    pub const fn operation(&self) -> TerminalOperation {
        self.operation
    }
    #[must_use]
    pub fn binding(&self) -> InstanceBinding {
        let mut binding = self.workspace.binding();
        binding.operation_ref = Some(format!("pty-session-v1:{}", self.operation.capability_id()));
        binding
    }
    #[must_use]
    pub fn verifier(&self) -> ProcessExecuteVerifier {
        ProcessExecuteVerifier::terminal(self.binding(), self.operation)
    }
    #[must_use]
    pub fn accepts_normalized_material(&self, input: &Value) -> bool {
        if !terminal_supported() {
            return false;
        }
        if self.operation == TerminalOperation::Start {
            return crate::execution::accepts_normalized_material(input, &self.workspace);
        }
        let Some(object) = input.as_object() else {
            return false;
        };
        if object
            .get("sessionId")
            .and_then(Value::as_str)
            .is_none_or(|id| resolve_terminal_session(id).is_err())
        {
            return false;
        }
        match self.operation {
            TerminalOperation::Start => unreachable!(),
            TerminalOperation::Read => {
                object.len() == 4
                    && input["offset"].as_u64().is_some()
                    && input["maxBytes"]
                        .as_u64()
                        .is_some_and(|n| (1..=4096).contains(&n))
                    && input["waitMs"].as_u64().is_some_and(|n| n <= 1000)
            }
            TerminalOperation::Write => {
                object.len() == 2
                    && input["input"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 1024)
            }
            TerminalOperation::Terminate => object.len() == 1,
        }
    }
}
impl EffectAdapter for TerminalAdapter {
    fn prepare(
        &mut self,
        request: AdapterPrepareRequest<'_>,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        let intent = request.intent();
        let instance = request.instance();
        if intent.invocation.capability_id != self.operation.capability_id()
            || intent.invocation.contract_version != TERMINAL_VERSION
            || instance.definition.capability_id != self.operation.capability_id()
            || instance.definition.contract_version != TERMINAL_VERSION
            || intent.invocation.instance_id != instance.instance_id
            || instance.binding != self.binding()
            || !crate::execution::supports_instance_features(&instance.features)
            || intent.effect_class != xgen_workgraph::EffectClass::NonIdempotent
            || intent.idempotency_key.as_deref().is_none_or(str::is_empty)
        {
            return Err(AdapterPrepareFailure::UnsupportedProtocol);
        }
        if !self.accepts_normalized_material(request.normalized_arguments()) {
            return Err(AdapterPrepareFailure::InvalidMaterial);
        }
        let key = intent.idempotency_key.as_deref().expect("validated key");
        let handle = format!(
            "pty-{}",
            crate::execution::sha256_digest(key.as_bytes()).trim_start_matches("sha256:")
        );
        Ok(Box::new(PreparedTerminal {
            adapter: self.clone(),
            input: request.normalized_arguments().clone(),
            handle,
        }))
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
struct PreparedTerminal {
    adapter: TerminalAdapter,
    input: Value,
    handle: String,
}
impl PreparedAdapterInvocation for PreparedTerminal {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        #[cfg(target_os = "linux")]
        let output = self.adapter.workspace.terminals.execute(
            self.adapter.operation,
            &self.input,
            &self.handle,
            &self.adapter.workspace,
        );
        #[cfg(not(target_os = "linux"))]
        let output = {
            let _ = (&self.adapter, &self.input);
            snapshot(&self.handle, "session_lost")
        };
        if output["state"] == "cleanup_failed" {
            return AdapterExecutionObservation::Unknown {
                reason: xgen_runtime::AdapterExecutionUnknownReason::TransportOutcomeUnknown,
            };
        }
        let digest =
            crate::execution::canonical_output_digest(&output).expect("finite terminal output");
        AdapterExecutionObservation::SucceededWithOutput {
            evidence_digest: AdapterEvidenceDigest::new(digest).expect("SHA-256"),
            output: AdapterToolOutput::new(output),
        }
    }
}
pub(crate) fn snapshot(handle: &str, state: &str) -> Value {
    json!({"sessionId":handle,"state":state,"exitCode":null,"output":"","baseOffset":0,"nextOffset":0,"truncated":false,"acceptedBytes":0})
}
pub(crate) fn inspect_output(
    output: &Value,
    expected: Option<&str>,
) -> Result<crate::verifier::InspectedOutput, ()> {
    let object = output
        .as_object()
        .filter(|object| object.len() == 8)
        .ok_or(())?;
    resolve_terminal_session(object.get("sessionId").and_then(Value::as_str).ok_or(())?)
        .map_err(|_| ())?;
    let state = output["state"].as_str().ok_or(())?;
    if !matches!(
        state,
        "running"
            | "stopping"
            | "draining"
            | "exited"
            | "terminated"
            | "timed_out"
            | "launch_failed"
            | "session_lost"
            | "session_limit"
            | "input_partial"
            | "input_failed"
            | "offset_invalid"
    ) {
        return Err(());
    }
    let code = match &output["exitCode"] {
        Value::Null => None,
        value => Some(
            value
                .as_u64()
                .filter(|n| u32::try_from(*n).is_ok())
                .ok_or(())?,
        ),
    };
    if (state == "exited" && code.is_none())
        || (matches!(
            state,
            "running" | "launch_failed" | "session_lost" | "session_limit"
        ) && code.is_some())
    {
        return Err(());
    }
    output["output"]
        .as_str()
        .filter(|text| text.len() <= 12288)
        .ok_or(())?;
    let base = output["baseOffset"].as_u64().ok_or(())?;
    let next = output["nextOffset"].as_u64().ok_or(())?;
    if next < base {
        return Err(());
    }
    output["truncated"].as_bool().ok_or(())?;
    output["acceptedBytes"]
        .as_u64()
        .filter(|n| *n <= 1024)
        .ok_or(())?;
    let bytes = serde_jcs::to_vec(output).map_err(|_| ())?;
    if bytes.len() > 32768 {
        return Err(());
    }
    let digest = crate::execution::sha256_digest(&bytes);
    if expected.is_some_and(|expected| expected != digest) {
        return Err(());
    }
    Ok(crate::verifier::InspectedOutput {
        digest,
        canonical_size_bytes: u64::try_from(bytes.len()).map_err(|_| ())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_shape_and_digest_fail_closed() {
        let id = format!("pty-{}", "a".repeat(64));
        let value = snapshot(&id, "session_lost");
        assert!(inspect_output(&value, None).is_ok());
        for (field, invalid) in [
            ("sessionId", json!("123")),
            ("state", json!("arbitrary")),
            ("exitCode", json!(-1)),
            ("nextOffset", json!(-1)),
            ("acceptedBytes", json!(1025)),
            ("output", json!("x".repeat(12289))),
            ("extra", json!(true)),
        ] {
            let mut forged = value.clone();
            forged[field] = invalid;
            assert!(inspect_output(&forged, None).is_err());
        }
        assert!(inspect_output(&value, Some(&format!("sha256:{}", "0".repeat(64)))).is_err());
        let mut invalid = value;
        invalid["state"] = json!("exited");
        assert!(inspect_output(&invalid, None).is_err());
    }
}
