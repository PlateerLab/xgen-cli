//! Host-owned execution facts are projected before the durable completion is committed.
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xgen_adapter_process::PROCESS_EXECUTE_CAPABILITY_ID;
use xgen_local_store::{RunStore, SqliteRunStore};
use xgen_provider_openai::OpenAiPlanner;
use xgen_runtime::{PlanProposal, PlannerCallRequest, PlannerPort, PlannerPortFailure};
use xgen_workgraph::{RunEventBody, validate_completion_summary_candidate};

use crate::material_catalog::RunMaterialCatalog;

pub(crate) struct ReceiptReportPlanner {
    inner: OpenAiPlanner,
    database: PathBuf,
    catalog: Option<RunMaterialCatalog>,
}

impl ReceiptReportPlanner {
    pub(crate) fn new(
        inner: OpenAiPlanner,
        database: &Path,
        material: &Path,
        run_id: &str,
    ) -> Result<Self, ()> {
        Ok(Self {
            inner,
            database: database.to_path_buf(),
            catalog: if material.exists() {
                Some(
                    RunMaterialCatalog::open_existing_read_only(material, run_id)
                        .map_err(|_| ())?,
                )
            } else {
                None
            },
        })
    }

    fn project(
        &self,
        request: &PlannerCallRequest<'_>,
        summary: &str,
    ) -> Result<String, PlannerPortFailure> {
        // Read-only preflight can return a private snapshot. Open it after this
        // call has been reserved, not when the planner is constructed.
        let store = SqliteRunStore::open_existing_read_only(&self.database)
            .map_err(|_| PlannerPortFailure::InvalidResponse)?;
        let invalid = || PlannerPortFailure::InvalidResponse;
        let (snapshot, mut receipts) = store
            .load_with_execution_receipts()
            .map_err(|_| invalid())?;
        let snapshot = snapshot.ok_or_else(invalid)?;
        let last = snapshot.records.last().ok_or_else(invalid)?;
        if last.sequence
            != request
                .context()
                .journal_sequence()
                .checked_add(1)
                .ok_or_else(invalid)?
            || last.previous_digest.as_deref() != Some(request.context().journal_head_digest())
            || !matches!(&last.event.body, RunEventBody::ModelCallReserved { reservation }
                if reservation.call_id() == request.call_id())
        {
            return Err(invalid());
        }
        // Order by actual starts, never Step IDs, proposal order, or model-written claims.
        let starts: std::collections::BTreeMap<_, _> = snapshot
            .records
            .iter()
            .filter_map(|record| {
                if let RunEventBody::EffectExecutionStarted { effect_id, .. } = &record.event.body {
                    Some((effect_id.as_str(), record.sequence))
                } else {
                    None
                }
            })
            .collect();
        receipts.sort_by_key(|receipt| {
            snapshot
                .state
                .steps
                .get(&receipt.step_id)
                .and_then(|step| step.intent.as_ref())
                .and_then(|intent| starts.get(intent.effect_id.as_str()))
                .copied()
        });
        let mut commands = Vec::new();
        for receipt in receipts {
            if receipt.capability.capability_id != PROCESS_EXECUTE_CAPABILITY_ID {
                continue;
            }
            let intent = snapshot
                .state
                .steps
                .get(&receipt.step_id)
                .and_then(|step| step.intent.as_ref())
                .ok_or_else(invalid)?;
            let output = store
                .load_tool_output(&intent.effect_id)
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            if output.output_digest() != receipt.output_digest {
                return Err(invalid());
            }
            let planned = store
                .load_planned_invocation(&receipt.step_id)
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            let arguments = self
                .catalog
                .as_ref()
                .ok_or_else(invalid)?
                .process_arguments(planned.reference(), &receipt.step_id, &receipt.input_digest)
                .map_err(|_| invalid())?;
            let executable = arguments["executable"]
                .as_str()
                .and_then(|s| s.rsplit_once("/executables/"))
                .map(|(_, id)| id)
                .filter(|id| !id.is_empty())
                .ok_or_else(invalid)?;
            let parameters = arguments["args"].as_array().ok_or_else(invalid)?;
            let mut argv = vec![Value::String(executable.to_owned())];
            for argument in parameters {
                argv.push(Value::String(
                    argument.as_str().ok_or_else(invalid)?.to_owned(),
                ));
            }
            let exit_code = output.output()["exitCode"].as_i64().ok_or_else(invalid)?;
            let sequence = starts.get(intent.effect_id.as_str()).ok_or_else(invalid)?;
            commands.push(
                json!({"argv":argv,"exit_code":exit_code,"execution_sequence":sequence,
                "receipt_id":receipt.receipt_id,"step_id":receipt.step_id}),
            );
        }
        let response: Value = serde_json::from_str(summary).map_err(|_| invalid())?;
        let result = serde_jcs::to_string(
            &json!({"format_version":1,"response":response,"commands":commands}),
        )
        .map_err(|_| invalid())?;
        validate_completion_summary_candidate(&result).map_err(|_| invalid())?;
        Ok(result)
    }
}

impl PlannerPort for ReceiptReportPlanner {
    fn planner_id(&self) -> &str {
        self.inner.planner_id()
    }
    fn request_profile_digest(&self) -> &str {
        self.inner.request_profile_digest()
    }
    fn plan(
        &mut self,
        request: &PlannerCallRequest<'_>,
    ) -> Result<PlanProposal, PlannerPortFailure> {
        match self.inner.plan(request)? {
            PlanProposal::CompletionCandidate { summary } => Ok(
                PlanProposal::completion_candidate(self.project(request, &summary)?),
            ),
            PlanProposal::ResponseCandidate { summary } => Ok(PlanProposal::response_candidate(
                self.project(request, &summary)?,
            )),
            other @ PlanProposal::Plan { .. } => Ok(other),
        }
    }
}
