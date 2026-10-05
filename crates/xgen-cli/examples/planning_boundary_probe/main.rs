//! Offline contract probe. Never included in the product binary.
use clap::Parser;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Write, num::NonZeroU32, path::PathBuf};
use xgen_cli::{
    ApprovalDecision, ApprovalPort, ApprovalPortFailure, DriverOutcome, DriverProgress,
    DriverProgressControl, PlannedRouteFailure, PlannedRoutePort, RunDriver,
};
use xgen_domain::{
    Architecture, CapabilityRef, DataBoundary, GrantLifetime, OperatingSystem, Platform,
    PolicySource, PolicySourceKind, ProtocolDocument, TrustLevel, VerificationResult,
};
use xgen_local_store::{ExpectedHead, RunStore, SqliteRunStore};
use xgen_policy::{
    PolicyAllowance, PolicyContribution, PolicyInputs, ResolvedPermissionRequest,
    ResourceResolutionFailure, ResourceResolver,
};
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure,
    AdapterPrepareRequest, AdapterReconcileRequest, AdapterReconciliationObservation,
    AdapterToolOutput, AgentLoop, CapabilityRegistry, EffectAdapter, EffectAdapterRegistry,
    EffectVerifier, EffectVerifierRegistry, EventFactory, EventFactoryError, EventMetadata,
    InvocationMaterialProvider, LocalRunLease, MaterialProviderFailure, MaterialProviderRegistry,
    PlanDependency, PlanMaterializationRequest, PlanMaterializer, PlanMaterializerFailure,
    PlanProposal, PlannerCallRequest, PlannerPort, PlannerPortFailure, PreparedAdapterInvocation,
    ProposedPlanStep, RequiredRouteFeatures, RouteRequest, RuleVerificationObservation,
    VerificationPortFailure, VerificationReport, VerificationRequest, VerifiedArtifactDescriptor,
    VerifierOutputDigest,
};
use xgen_workgraph::{
    AgentLoopBudget, ReconstructableMaterialReference, RunEvent, RunEventBody, RunState,
};

const RUN: &str = "planning-boundary-probe";
const PROFILE: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
#[derive(Parser)]
struct Args {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    fixture: PathBuf,
    #[arg(long, default_value_t = 1)]
    packet_size: usize,
    #[arg(long, default_value = "none")]
    fault: String,
    #[arg(long)]
    query: bool,
    #[arg(long)]
    discard_model_call: bool,
    #[arg(long)]
    catalog_drift: bool,
    #[arg(long)]
    fail_operation: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    key: String,
    resource: String,
    value: Value,
    #[serde(default)]
    depends_on: Vec<String>,
}
#[derive(Deserialize)]
struct Plan {
    initial: Vec<Operation>,
}
fn digest(value: &Value) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::from("sha256:");
    for byte in Sha256::digest(serde_jcs::to_vec(value).unwrap()) {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}
fn connect(args: &Args) -> Connection {
    Connection::open(args.root.join("sink.sqlite3")).unwrap()
}
fn rendezvous(fault: &str, boundary: &str) {
    if fault == boundary {
        println!(
            "{}",
            json!({"event":"rendezvous", "boundary":boundary, "run_id":RUN})
        );
        std::io::stdout().flush().unwrap();
        // The parent must terminate this process; EOF is never permission to continue.
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        std::process::exit(70);
    }
}
struct Events;
impl EventFactory for Events {
    fn create_metadata(&mut self, state: &RunState) -> Result<EventMetadata, EventFactoryError> {
        Ok(EventMetadata {
            event_id: format!("probe-event-{}", state.journal_sequence + 1),
            recorded_at: "2026-10-05T00:00:00Z".into(),
        })
    }
}
struct Resolver;
impl ResourceResolver for Resolver {
    fn resolve(&self, scope: &str, resource: &str) -> Result<String, ResourceResolutionFailure> {
        if scope == "probe.write"
            && resource.starts_with("probe:")
            && resource.len() <= 64
            && resource
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":/-_".contains(&b))
        {
            Ok(resource.to_owned())
        } else {
            Err(ResourceResolutionFailure::OutsideHostBoundary)
        }
    }
}
struct Planner {
    plan: Plan,
    packet: usize,
    cap: CapabilityRef,
    profile: String,
    db: PathBuf,
    fault: String,
}
impl PlannerPort for Planner {
    fn planner_id(&self) -> &'static str {
        "xgen.test.planning-boundary"
    }
    fn request_profile_digest(&self) -> &str {
        &self.profile
    }
    fn plan(
        &mut self,
        request: &PlannerCallRequest<'_>,
    ) -> Result<PlanProposal, PlannerPortFailure> {
        Connection::open(&self.db)
            .unwrap()
            .execute("INSERT INTO calls(kind) VALUES ('planner')", [])
            .unwrap();
        rendezvous(&self.fault, "model_reserved");
        let mut visible = self.plan.initial.clone();
        let mut completed = BTreeMap::new();
        for output in request.context().tool_outputs() {
            completed.insert(
                output.output()["operation"].as_str().unwrap().to_owned(),
                output.step_id().to_owned(),
            );
            if let Some(next) = output.output()["value"].get("next") {
                visible.extend(
                    serde_json::from_value::<Vec<Operation>>(next.clone())
                        .map_err(|_| PlannerPortFailure::Unavailable)?,
                );
            }
        }
        let mut selected: Vec<String> = Vec::new();
        let mut steps = Vec::new();
        for op in visible {
            if completed.contains_key(&op.key) || selected.contains(&op.key) {
                continue;
            }
            if !op
                .depends_on
                .iter()
                .all(|key| completed.contains_key(key) || selected.contains(key))
            {
                continue;
            }
            let dependencies = op
                .depends_on
                .iter()
                .map(|key| {
                    completed.get(key).map_or_else(
                        || PlanDependency::proposed(key.clone()),
                        |step| PlanDependency::existing(step.clone()),
                    )
                })
                .collect();
            steps.push(ProposedPlanStep::new(
                op.key.clone(),
                "Apply one bounded fixture operation",
                dependencies,
                self.cap.clone(),
                json!({"operation":op.key,"resource":op.resource,"value":op.value}),
            ));
            selected.push(op.key);
            if steps.len() == self.packet {
                break;
            }
        }
        if steps.is_empty() {
            Ok(PlanProposal::completion_candidate(
                "Fixture operations completed.",
            ))
        } else {
            Ok(PlanProposal::plan(steps))
        }
    }
}
struct Recipes(PathBuf);
impl PlanMaterializer for Recipes {
    fn materialize(
        &mut self,
        request: PlanMaterializationRequest<'_>,
    ) -> Result<ReconstructableMaterialReference, PlanMaterializerFailure> {
        let id = digest(request.normalized_arguments()).replace(':', "-");
        Connection::open(&self.0)
            .map_err(|_| PlanMaterializerFailure::PersistenceFailed)?
            .execute(
                "INSERT OR IGNORE INTO recipes(id,args) VALUES (?1,?2)",
                params![id, request.normalized_arguments().to_string()],
            )
            .map_err(|_| PlanMaterializerFailure::PersistenceFailed)?;
        ReconstructableMaterialReference::new("probe-recipes", id, "v1")
            .map_err(|_| PlanMaterializerFailure::PersistenceFailed)
    }
}
impl InvocationMaterialProvider for Recipes {
    fn reconstruct(
        &mut self,
        reference_id: &str,
        revision: &str,
    ) -> Result<Value, MaterialProviderFailure> {
        if revision != "v1" {
            return Err(MaterialProviderFailure::RevisionChanged);
        }
        let raw: String = Connection::open(&self.0)
            .map_err(|_| MaterialProviderFailure::NotFound)?
            .query_row(
                "SELECT args FROM recipes WHERE id=?1",
                [reference_id],
                |row| row.get(0),
            )
            .map_err(|_| MaterialProviderFailure::NotFound)?;
        serde_json::from_str(&raw).map_err(|_| MaterialProviderFailure::NotFound)
    }
}
struct Sink {
    db: PathBuf,
    fault: String,
    fail_operation: Option<String>,
}
struct Invocation {
    db: PathBuf,
    fault: String,
    key: String,
    output: Value,
    fail_operation: Option<String>,
}
impl PreparedAdapterInvocation for Invocation {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        let db = Connection::open(&self.db).unwrap();
        db.execute("INSERT INTO calls(kind) VALUES ('execute')", [])
            .unwrap();
        db.execute(
            "INSERT INTO attempts(effect_key,output) VALUES (?1,?2)",
            params![self.key, self.output.to_string()],
        )
        .unwrap();
        if self.fail_operation.as_deref() == self.output["operation"].as_str() {
            return AdapterExecutionObservation::Failed {
                evidence_digest: AdapterEvidenceDigest::new(digest(&self.output)).unwrap(),
            };
        }
        // No UNIQUE constraint or deduplication: an unsafe replay must remain visible.
        db.execute(
            "INSERT INTO applied(effect_key,output) VALUES (?1,?2)",
            params![self.key, self.output.to_string()],
        )
        .unwrap();
        rendezvous(&self.fault, "sink_applied");
        AdapterExecutionObservation::SucceededWithOutput {
            evidence_digest: AdapterEvidenceDigest::new(digest(&self.output)).unwrap(),
            output: AdapterToolOutput::new(self.output),
        }
    }
}
impl EffectAdapter for Sink {
    fn prepare(
        &mut self,
        request: AdapterPrepareRequest<'_>,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        Ok(Box::new(Invocation {
            db: self.db.clone(),
            fault: self.fault.clone(),
            fail_operation: self.fail_operation.clone(),
            key: request.intent().idempotency_key.clone().unwrap(),
            output: request.normalized_arguments().clone(),
        }))
    }
    fn reconcile(
        &mut self,
        request: AdapterReconcileRequest<'_>,
    ) -> AdapterReconciliationObservation {
        let db = Connection::open(&self.db).unwrap();
        db.execute("INSERT INTO calls(kind) VALUES ('reconcile')", [])
            .unwrap();
        let output: String = db
            .query_row(
                "SELECT output FROM applied WHERE effect_key=?1",
                [request.intent().idempotency_key.as_deref().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        AdapterReconciliationObservation::Applied {
            evidence_digest: AdapterEvidenceDigest::new(digest(
                &serde_json::from_str(&output).unwrap(),
            ))
            .unwrap(),
        }
    }
}
struct Verifier {
    db: PathBuf,
    fault: String,
}
impl EffectVerifier for Verifier {
    fn verify(
        &mut self,
        request: VerificationRequest<'_>,
    ) -> Result<VerificationReport, VerificationPortFailure> {
        let db = Connection::open(&self.db).unwrap();
        db.execute("INSERT INTO calls(kind) VALUES ('verifier')", [])
            .unwrap();
        rendezvous(&self.fault, "before_receipt");
        let output = request
            .tool_output()
            .ok_or(VerificationPortFailure::EvidenceUnavailable)?;
        let actual: String = db
            .query_row(
                "SELECT output FROM applied WHERE effect_key=?1",
                [request.intent().idempotency_key.as_deref().unwrap()],
                |row| row.get(0),
            )
            .map_err(|_| VerificationPortFailure::EvidenceUnavailable)?;
        if serde_json::from_str::<Value>(&actual).unwrap() != *output.output() {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
        VerificationReport::new(
            VerifierOutputDigest::new(output.output_digest()).unwrap(),
            request
                .definition()
                .spec
                .verification
                .iter()
                .map(|rule| {
                    RuleVerificationObservation::new(
                        rule.strategy,
                        VerificationResult::Passed,
                        Some(AdapterEvidenceDigest::new(digest(output.output())).unwrap()),
                    )
                })
                .collect(),
        )
        .with_artifacts(vec![
            VerifiedArtifactDescriptor::new(
                "probe-output",
                None::<String>,
                "application/json",
                u64::try_from(serde_jcs::to_vec(output.output()).unwrap().len()).unwrap(),
                output.output_digest(),
            )
            .unwrap(),
        ])
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)
    }
}
struct Route {
    cap: CapabilityRef,
    instance: String,
    platform: Platform,
}
impl PlannedRoutePort for Route {
    fn route_for(&mut self, _: &RunState, _: &str) -> Result<RouteRequest, PlannedRouteFailure> {
        Ok(RouteRequest {
            capability: self.cap.clone(),
            target_platform: self.platform.clone(),
            required_features: RequiredRouteFeatures {
                execution_style: xgen_domain::ExecutionStyle::Sync,
                cancellation: false,
                idempotency_key: true,
                idempotency_query: false,
            },
            allowed_trust_levels: vec![TrustLevel::Verified],
            allowed_data_boundaries: vec![DataBoundary::Local],
            trust_preference: vec![],
            data_boundary_preference: vec![],
            preferred_instance_ids: vec![],
            pinned_instance_id: Some(self.instance.clone()),
        })
    }
}
struct Approval;
impl ApprovalPort for Approval {
    fn decide(
        &mut self,
        request: &ResolvedPermissionRequest,
    ) -> Result<ApprovalDecision, ApprovalPortFailure> {
        let allowance = || {
            PolicyAllowance::from_trusted_evaluation(
                request.requested_scopes().iter().cloned(),
                request.resources().iter().cloned(),
                request.critical_actions().iter().copied(),
                [GrantLifetime::Once],
            )
        };
        let source = |kind, id: &str| PolicySource {
            kind,
            id: id.into(),
            digest: PROFILE.into(),
        };
        Ok(ApprovalDecision::Approved(Box::new(PolicyInputs::local(
            request,
            PolicyContribution::allow(source(PolicySourceKind::Host, "probe-host"), allowance()),
            PolicyContribution::allow(
                source(PolicySourceKind::UserProfile, "probe-user"),
                allowance(),
            ),
        ))))
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("probe setup failed: {error}");
        std::process::exit(1);
    }
}
#[allow(clippy::too_many_lines)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if ![1, 4].contains(&args.packet_size) {
        return Err("packet size must be 1 or 4".into());
    }
    std::fs::create_dir_all(&args.root)?;
    let lease = LocalRunLease::try_acquire(RUN, args.root.join("run.lock"))?;
    let db = connect(&args);
    db.execute_batch("CREATE TABLE IF NOT EXISTS recipes(id TEXT PRIMARY KEY,args TEXT NOT NULL); CREATE TABLE IF NOT EXISTS attempts(id INTEGER PRIMARY KEY,effect_key TEXT NOT NULL,output TEXT NOT NULL); CREATE TABLE IF NOT EXISTS applied(id INTEGER PRIMARY KEY,effect_key TEXT NOT NULL,output TEXT NOT NULL); CREATE TABLE IF NOT EXISTS calls(id INTEGER PRIMARY KEY,kind TEXT NOT NULL);")?;
    let fixture: Value = serde_json::from_slice(&std::fs::read(&args.fixture)?)?;
    let plan: Plan = serde_json::from_value(fixture["plan"].clone())?;
    let config =
        json!({"fixture":digest(&fixture),"packet_size":args.packet_size,"query":args.query});
    let manifest = args.root.join("manifest.json");
    let profile_matches = if manifest.exists() {
        serde_json::from_slice::<Value>(&std::fs::read(&manifest)?)? == config
    } else {
        std::fs::write(&manifest, config.to_string())?;
        true
    };
    let mut definition: Value = serde_json::from_str(include_str!(
        "../../../../protocol/fixtures/v1alpha1/valid/capability-definition.non-idempotent-durable-output.json"
    ))?;
    let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["operation","resource","value"],"properties":{"operation":{"type":"string","minLength":1,"maxLength":64},"resource":{"type":"string","minLength":1,"maxLength":64},"value":{}},"additionalProperties":false});
    if args.catalog_drift {
        definition["spec"]["summary"] = json!("Changed catalog revision");
    }
    definition["spec"]["inputSchema"] = schema.clone();
    definition["spec"]["outputSchema"] = schema;
    definition["spec"]["effect"]["resourceSelectors"] =
        json!([{"scope":"probe.write","argumentPointer":"/resource"}]);
    let ProtocolDocument::CapabilityDefinition(definition) = serde_json::from_value(definition)?
    else {
        unreachable!()
    };
    let cap = CapabilityRef {
        capability_id: definition.metadata.id.clone(),
        contract_version: definition.metadata.contract_version.clone(),
    };
    let ProtocolDocument::CapabilityInstance(mut instance) = serde_json::from_str(include_str!(
        "../../../../protocol/fixtures/v1alpha1/valid/capability-instance.local-fs.json"
    ))?
    else {
        unreachable!()
    };
    instance.definition = cap.clone();
    instance.features.cancellable = false;
    instance.features.idempotency_query = args.query;
    instance.platform.os = match std::env::consts::OS {
        "linux" => OperatingSystem::Linux,
        "macos" => OperatingSystem::Macos,
        "windows" => OperatingSystem::Windows,
        _ => return Err("unsupported OS".into()),
    };
    instance.platform.arch = match std::env::consts::ARCH {
        "x86_64" => Architecture::X86_64,
        "aarch64" => Architecture::Aarch64,
        _ => return Err("unsupported architecture".into()),
    };
    let mut routes = Route {
        cap: cap.clone(),
        instance: instance.instance_id.clone(),
        platform: instance.platform.clone(),
    };
    let mut capabilities = CapabilityRegistry::new();
    capabilities.register_schema_validated_definition(*definition)?;
    capabilities.register_schema_validated_instance(*instance.clone())?;
    let sink_path = args.root.join("sink.sqlite3");
    let mut adapters = EffectAdapterRegistry::new();
    adapters.register(
        &instance.binding,
        Sink {
            fail_operation: args.fail_operation.clone(),
            db: sink_path.clone(),
            fault: args.fault.clone(),
        },
    )?;
    let mut verifiers = EffectVerifierRegistry::new();
    verifiers.register(
        &instance.binding,
        Verifier {
            db: sink_path.clone(),
            fault: args.fault.clone(),
        },
    )?;
    let mut providers = MaterialProviderRegistry::new();
    providers.register("probe-recipes", Recipes(sink_path.clone()))?;
    let mut materializer = Recipes(sink_path.clone());
    let mut planner = Planner {
        plan,
        packet: args.packet_size,
        cap,
        profile: digest(&config),
        db: sink_path,
        fault: args.fault.clone(),
    };
    let mut store = SqliteRunStore::open(args.root.join("run.sqlite3"))?;
    if store.load_current()?.is_none() {
        store.append(
            ExpectedHead::Empty,
            RunEvent {
                event_id: "probe-seed".into(),
                run_id: RUN.into(),
                authority: "local:probe".into(),
                authority_epoch: 1,
                recorded_at: "2026-10-05T00:00:00Z".into(),
                body: RunEventBody::RunCreated {
                    goal: "Apply bounded offline fixture operations".into(),
                },
            },
        )?;
    }
    let agent = AgentLoop::new(AgentLoopBudget::new(32, 32, 32, 524_288)?);
    let mut events = Events;
    if args.discard_model_call {
        let state = store.load_current()?.unwrap();
        let call = state
            .agent_loop
            .as_ref()
            .and_then(|s| s.model_calls.as_ref())
            .and_then(|s| s.active_call.as_ref())
            .ok_or("no active model call")?;
        agent.abandon_model_call(&mut store, &mut events, &lease, call.reservation.call_id())?;
    }
    let mut observer = |progress| {
        match progress {
            DriverProgress::PlanCommitted => rendezvous(&args.fault, "plan_committed"),
            DriverProgress::ActionAuthorized { .. } => rendezvous(&args.fault, "authorized"),
            _ => {}
        }
        DriverProgressControl::Continue
    };
    let result = if profile_matches {
        RunDriver::new(agent, NonZeroU32::new(256).unwrap())
            .drive_until_pause_with_model_egress_observed(
                &mut store,
                &mut events,
                &lease,
                &capabilities,
                &Resolver,
                &mut planner,
                &mut materializer,
                &mut providers,
                &mut adapters,
                &mut verifiers,
                &mut routes,
                &mut Approval,
                true,
                &mut observer,
            )
    } else {
        Ok(DriverOutcome::ModelEgressRequired)
    };
    let (status, completed) = if profile_matches {
        match result {
            Ok(DriverOutcome::CompletionCandidate { .. }) => ("completed", true),
            Ok(DriverOutcome::ModelCallRecoveryRequired { .. }) => {
                ("model_call_recovery_required", false)
            }
            Ok(DriverOutcome::Quiescent(_)) => ("quiescent", false),
            Ok(_) => ("paused", false),
            Err(_) => ("driver_error", false),
        }
    } else {
        ("profile_mismatch", false)
    };
    let snapshot = store.load()?.unwrap();
    let applied: Vec<Value> = db
        .prepare("SELECT output FROM applied ORDER BY id")?
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect();
    let counts: BTreeMap<String, i64> = db
        .prepare("SELECT kind,count(*) FROM calls GROUP BY kind")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let duplicates:i64 = db.query_row("SELECT coalesce(sum(n-1),0) FROM (SELECT count(*) AS n FROM applied GROUP BY effect_key HAVING n>1)",[],|row| row.get(0))?;
    println!(
        "{}",
        json!({"event":"report","status":status,"completed":completed,"run_id":RUN,"applied":applied,"duplicate_effects":duplicates,"calls":counts,"journal_events":snapshot.records.len(),"receipts":store.load_execution_receipts()?.len(),"state":snapshot.state})
    );
    Ok(())
}
