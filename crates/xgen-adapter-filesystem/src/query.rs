use std::fmt::Write as _;
use std::io::Read as _;
use std::sync::Arc;

use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _, OpenOptionsSyncExt as _};
use cap_std::fs::{Dir, File, OpenOptions};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use xgen_domain::{InstanceFeatures, VerificationResult, VerificationStrategy};
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure,
    AdapterPrepareRequest, AdapterReconcileRequest, AdapterReconciliationInconclusiveReason,
    AdapterReconciliationObservation, AdapterToolOutput, EffectAdapter, EffectVerifier,
    PreparedAdapterInvocation, RuleVerificationObservation, StepFailureCode,
    VerificationPortFailure, VerificationReport, VerificationRequest, VerifiedArtifactDescriptor,
    VerifierOutputDigest,
};
use xgen_workgraph::EffectClass;

use crate::path::{
    OpenErrorClass, RelativePath, canonical_resource, classify_open_error, parse_canonical,
};
use crate::{
    LIST_DIRECTORY_CAPABILITY_ID, LIST_DIRECTORY_CONTRACT_VERSION, READ_TEXT_LINES_CAPABILITY_ID,
    READ_TEXT_LINES_CONTRACT_VERSION, SEARCH_TEXT_CAPABILITY_ID, SEARCH_TEXT_CONTRACT_VERSION,
    STAT_CAPABILITY_ID, STAT_CONTRACT_VERSION, WorkspaceRoot,
};

/// Maximum entries returned by one list-directory observation.
pub const MAX_LIST_DIRECTORY_ENTRIES: usize = 512;
/// Maximum portable entries scanned in one directory before the operation fails closed.
pub const MAX_DIRECTORY_SCAN_ENTRIES: usize = 4_096;
/// Maximum literal query size accepted by search-text.
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
/// Maximum Unicode scalar values accepted by the public search schema and host validation.
pub const MAX_SEARCH_QUERY_UNICODE_SCALARS: usize = 64;
/// Maximum matches returned by one search-text observation.
pub const MAX_SEARCH_MATCHES: usize = 128;
/// Maximum entries visited recursively by one search-text observation.
pub const MAX_SEARCH_VISITED_ENTRIES: usize = 4_096;
/// Maximum bytes read from one candidate search file.
pub const MAX_SEARCH_FILE_BYTES: usize = 256 * 1024;
/// Maximum aggregate candidate bytes read by one search-text observation.
pub const MAX_SEARCH_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// Maximum UTF-8 bytes retained in one match preview.
pub const MAX_SEARCH_PREVIEW_BYTES: usize = 512;
/// Maximum lines requested by one read-text-lines observation.
pub const MAX_READ_TEXT_LINES_COUNT: u64 = 2_000;
/// Maximum file size scanned by one read-text-lines observation.
pub const MAX_READ_TEXT_LINES_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum JSON-escaped bytes of the content returned by one read-text-lines observation.
pub const MAX_READ_TEXT_LINES_CONTENT_BYTES: usize = 64 * 1024;
/// Maximum canonical JCS bytes emitted by one query tool output, including its digest.
pub const MAX_QUERY_OUTPUT_CANONICAL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryOperation {
    ListDirectory,
    Stat,
    SearchText,
    ReadTextLines,
}

impl QueryOperation {
    const fn capability_id(self) -> &'static str {
        match self {
            Self::ListDirectory => LIST_DIRECTORY_CAPABILITY_ID,
            Self::Stat => STAT_CAPABILITY_ID,
            Self::SearchText => SEARCH_TEXT_CAPABILITY_ID,
            Self::ReadTextLines => READ_TEXT_LINES_CAPABILITY_ID,
        }
    }

    const fn contract_version(self) -> &'static str {
        match self {
            Self::ListDirectory => LIST_DIRECTORY_CONTRACT_VERSION,
            Self::Stat => STAT_CONTRACT_VERSION,
            Self::SearchText => SEARCH_TEXT_CONTRACT_VERSION,
            Self::ReadTextLines => READ_TEXT_LINES_CONTRACT_VERSION,
        }
    }

    const fn artifact_id(self) -> &'static str {
        match self {
            Self::ListDirectory => "list-directory-output",
            Self::Stat => "stat-output",
            Self::SearchText => "search-text-output",
            Self::ReadTextLines => "read-text-lines-output",
        }
    }

    const fn failure_domain(self) -> &'static str {
        match self {
            Self::ListDirectory => "xgeny.fs/list-directory/failure/v1",
            Self::Stat => "xgeny.fs/stat/failure/v1",
            Self::SearchText => "xgeny.fs/search-text/failure/v1",
            Self::ReadTextLines => "xgeny.fs/read-text-lines/failure/v1",
        }
    }

    const fn argument_count(self) -> usize {
        match self {
            Self::ListDirectory | Self::Stat => 1,
            Self::SearchText => 2,
            Self::ReadTextLines => 3,
        }
    }

    /// Output fields including the appended digest.
    const fn output_field_count(self) -> usize {
        match self {
            Self::ListDirectory | Self::Stat | Self::SearchText => 3,
            Self::ReadTextLines => 7,
        }
    }
}

/// Exact root-bound adapter for one bounded filesystem observation.
#[derive(Clone)]
pub struct FilesystemQueryAdapter {
    root: WorkspaceRoot,
    operation: QueryOperation,
}

impl FilesystemQueryAdapter {
    pub(crate) const fn list_directory(root: WorkspaceRoot) -> Self {
        Self {
            root,
            operation: QueryOperation::ListDirectory,
        }
    }

    pub(crate) const fn stat(root: WorkspaceRoot) -> Self {
        Self {
            root,
            operation: QueryOperation::Stat,
        }
    }

    pub(crate) const fn search_text(root: WorkspaceRoot) -> Self {
        Self {
            root,
            operation: QueryOperation::SearchText,
        }
    }

    pub(crate) const fn read_text_lines(root: WorkspaceRoot) -> Self {
        Self {
            root,
            operation: QueryOperation::ReadTextLines,
        }
    }

    /// Construct a verifier pinned to the same workspace and exact operation binding.
    #[must_use]
    pub fn verifier(&self) -> FilesystemQueryVerifier {
        FilesystemQueryVerifier {
            workspace_id: self.root.workspace_id.clone(),
            expected_binding: binding_for(&self.root, self.operation),
            operation: self.operation,
        }
    }
}

impl std::fmt::Debug for FilesystemQueryAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemQueryAdapter")
            .field("root", &"<preopened/redacted>")
            .field("operation", &self.operation)
            .finish()
    }
}

impl EffectAdapter for FilesystemQueryAdapter {
    fn prepare(
        &mut self,
        request: AdapterPrepareRequest<'_>,
    ) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
        let binding = binding_for(&self.root, self.operation);
        verify_contract(&request, &binding, self.operation)?;
        let arguments =
            parse_arguments(request.normalized_arguments(), &self.root, self.operation)?;
        Ok(Box::new(PreparedFilesystemQuery {
            directory: Arc::clone(&self.root.directory),
            relative_path: arguments.path,
            query: arguments.query,
            lines: arguments.lines,
            operation: self.operation,
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

/// Verifier for one exact filesystem query operation.
pub struct FilesystemQueryVerifier {
    workspace_id: crate::WorkspaceId,
    expected_binding: xgen_domain::InstanceBinding,
    operation: QueryOperation,
}

impl std::fmt::Debug for FilesystemQueryVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemQueryVerifier")
            .field("workspace_id", &"<redacted>")
            .field("expected_binding", &"<redacted>")
            .field("operation", &self.operation)
            .finish()
    }
}

impl EffectVerifier for FilesystemQueryVerifier {
    fn verify(
        &mut self,
        request: VerificationRequest<'_>,
    ) -> Result<VerificationReport, VerificationPortFailure> {
        verify_verifier_contract(&request, &self.expected_binding, self.operation)?;
        let output = request
            .tool_output()
            .ok_or(VerificationPortFailure::EvidenceUnavailable)?;
        let inspected = inspect_output(
            output.output(),
            request.outcome_evidence_digest().as_str(),
            &self.workspace_id,
            self.operation,
        )?;
        let evidence = AdapterEvidenceDigest::new(inspected.digest.clone())
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        let rules = request
            .definition()
            .spec
            .verification
            .iter()
            .map(|rule| {
                let result = match rule.strategy {
                    VerificationStrategy::OutputSchema => VerificationResult::Passed,
                    VerificationStrategy::Postcondition if inspected.postcondition => {
                        VerificationResult::Passed
                    }
                    VerificationStrategy::Postcondition => VerificationResult::Failed,
                    _ => return Err(VerificationPortFailure::UnsupportedStrategy),
                };
                Ok(RuleVerificationObservation::new(
                    rule.strategy,
                    result,
                    Some(
                        AdapterEvidenceDigest::new(evidence.as_str().to_owned())
                            .expect("validated digest remains canonical"),
                    ),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let artifact = VerifiedArtifactDescriptor::new(
            self.operation.artifact_id(),
            Option::<String>::None,
            "application/json",
            inspected.byte_size,
            inspected.digest,
        )
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        VerificationReport::new(
            VerifierOutputDigest::new(output.output_digest().to_owned())
                .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?,
            rules,
        )
        .with_artifacts(vec![artifact])
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)
    }
}

struct PreparedFilesystemQuery {
    directory: Arc<Dir>,
    relative_path: RelativePath,
    query: Option<String>,
    lines: Option<LineRange>,
    operation: QueryOperation,
}

impl std::fmt::Debug for PreparedFilesystemQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedFilesystemQuery")
            .field("directory", &"<preopened/redacted>")
            .field("relative_path", &"<redacted>")
            .field("query", &self.query.as_ref().map(|_| "<redacted>"))
            .field("lines", &self.lines)
            .field("operation", &self.operation)
            .finish()
    }
}

impl PreparedAdapterInvocation for PreparedFilesystemQuery {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        let observation = match self.operation {
            QueryOperation::ListDirectory => list_directory(&self.directory, &self.relative_path),
            QueryOperation::Stat => stat_path(&self.directory, &self.relative_path),
            QueryOperation::SearchText => search_text(
                &self.directory,
                &self.relative_path,
                self.query
                    .as_deref()
                    .expect("search preparation always retains a query"),
            ),
            QueryOperation::ReadTextLines => read_text_lines(
                &self.directory,
                &self.relative_path,
                self.lines
                    .expect("line-range preparation always retains a range"),
            ),
        };
        match observation.and_then(ObservedOutput::finish) {
            Ok(observation) => AdapterExecutionObservation::SucceededWithOutput {
                evidence_digest: AdapterEvidenceDigest::new(observation.digest)
                    .expect("SHA-256 output digest is canonical"),
                output: AdapterToolOutput::new(observation.output),
            },
            Err(failure) => AdapterExecutionObservation::Failed {
                evidence_digest: AdapterEvidenceDigest::new(
                    failure.evidence_digest(self.operation),
                )
                .expect("SHA-256 failure digest is canonical"),
                failure_code: failure.failure_code(),
            },
        }
    }
}

struct ParsedArguments {
    path: RelativePath,
    query: Option<String>,
    lines: Option<LineRange>,
}

/// One-based first line and requested line count of a read-text-lines observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineRange {
    start_line: u64,
    line_count: u64,
}

struct ObservedOutput {
    payload: Value,
}

struct FinishedOutput {
    output: Value,
    digest: String,
}

impl ObservedOutput {
    fn finish(self) -> Result<FinishedOutput, QueryFailure> {
        let canonical = serde_jcs::to_vec(&self.payload).map_err(|_| QueryFailure::Encode)?;
        let digest = sha256_digest(&canonical);
        let mut object = self
            .payload
            .as_object()
            .cloned()
            .ok_or(QueryFailure::Encode)?;
        object.insert("digest".to_owned(), Value::String(digest.clone()));
        let output = Value::Object(object);
        if serde_jcs::to_vec(&output)
            .map_err(|_| QueryFailure::Encode)?
            .len()
            > MAX_QUERY_OUTPUT_CANONICAL_BYTES
        {
            return Err(QueryFailure::Limit);
        }
        Ok(FinishedOutput { output, digest })
    }
}

fn bounded_collection_output(
    field: &'static str,
    candidates: Vec<Value>,
    mut truncated: bool,
) -> Result<ObservedOutput, QueryFailure> {
    let base = collection_output_size(field, &[], false)?;
    let mut projected_size = base;
    let mut retained = Vec::new();
    for candidate in candidates {
        let candidate_size = serde_jcs::to_vec(&candidate)
            .map_err(|_| QueryFailure::Encode)?
            .len();
        let separator = usize::from(!retained.is_empty());
        let Some(next_size) = projected_size
            .checked_add(separator)
            .and_then(|size| size.checked_add(candidate_size))
        else {
            truncated = true;
            break;
        };
        if next_size > MAX_QUERY_OUTPUT_CANONICAL_BYTES {
            truncated = true;
            break;
        }
        retained.push(candidate);
        projected_size = next_size;
    }
    let payload = collection_payload(field, retained, truncated);
    if collection_output_size_from_payload(&payload)? > MAX_QUERY_OUTPUT_CANONICAL_BYTES {
        return Err(QueryFailure::Limit);
    }
    Ok(ObservedOutput { payload })
}

fn collection_payload(field: &'static str, values: Vec<Value>, truncated: bool) -> Value {
    let mut payload = Map::new();
    payload.insert(field.to_owned(), Value::Array(values));
    payload.insert("truncated".to_owned(), Value::Bool(truncated));
    Value::Object(payload)
}

fn collection_output_size(
    field: &'static str,
    values: &[Value],
    truncated: bool,
) -> Result<usize, QueryFailure> {
    collection_output_size_from_payload(&collection_payload(field, values.to_vec(), truncated))
}

fn collection_output_size_from_payload(payload: &Value) -> Result<usize, QueryFailure> {
    let canonical = serde_jcs::to_vec(payload).map_err(|_| QueryFailure::Encode)?;
    let digest = sha256_digest(&canonical);
    let mut object = payload.as_object().cloned().ok_or(QueryFailure::Encode)?;
    object.insert("digest".to_owned(), Value::String(digest));
    serde_jcs::to_vec(&Value::Object(object))
        .map(|canonical| canonical.len())
        .map_err(|_| QueryFailure::Encode)
}

struct InspectedOutput {
    digest: String,
    byte_size: u64,
    postcondition: bool,
}

#[derive(Debug, Clone, Copy)]
enum QueryFailure {
    NotFound,
    Open,
    NotAFile,
    NotADirectory,
    TooLarge,
    Read,
    TooManyEntries,
    Limit,
    Encode,
    Decode,
}

impl QueryFailure {
    fn evidence_digest(self, operation: QueryOperation) -> String {
        let code = match self {
            Self::NotFound => "not-found",
            Self::Open => "open",
            Self::NotAFile => "not-a-file",
            Self::NotADirectory => "not-a-directory",
            Self::TooLarge => "too-large",
            Self::Read => "read",
            Self::TooManyEntries => "too-many-entries",
            Self::Limit => "limit",
            Self::Encode => "encode",
            Self::Decode => "decode",
        };
        sha256_digest(format!("{}/{code}", operation.failure_domain()).as_bytes())
    }

    const fn failure_code(self) -> StepFailureCode {
        match self {
            Self::NotFound => StepFailureCode::NotFound,
            Self::Open => StepFailureCode::Unavailable,
            Self::NotAFile => StepFailureCode::NotAFile,
            Self::NotADirectory => StepFailureCode::NotADirectory,
            Self::TooLarge => StepFailureCode::TooLarge,
            Self::Read | Self::Encode => StepFailureCode::IoError,
            Self::TooManyEntries | Self::Limit => StepFailureCode::LimitExceeded,
            Self::Decode => StepFailureCode::NotUtf8,
        }
    }
}

fn binding_for(root: &WorkspaceRoot, operation: QueryOperation) -> xgen_domain::InstanceBinding {
    match operation {
        QueryOperation::ListDirectory => root.list_directory_binding(),
        QueryOperation::Stat => root.stat_binding(),
        QueryOperation::SearchText => root.search_text_binding(),
        QueryOperation::ReadTextLines => root.read_text_lines_binding(),
    }
}

fn verify_contract(
    request: &AdapterPrepareRequest<'_>,
    expected_binding: &xgen_domain::InstanceBinding,
    operation: QueryOperation,
) -> Result<(), AdapterPrepareFailure> {
    let intent = request.intent();
    let instance = request.instance();
    if intent.invocation.capability_id != operation.capability_id()
        || intent.invocation.contract_version != operation.contract_version()
        || instance.definition.capability_id != operation.capability_id()
        || instance.definition.contract_version != operation.contract_version()
        || intent.invocation.instance_id != instance.instance_id
        || instance.binding != *expected_binding
        || !supports_instance_features(&instance.features)
        || intent.effect_class != EffectClass::ReadOnly
        || intent.idempotency_key.is_some()
    {
        return Err(AdapterPrepareFailure::UnsupportedProtocol);
    }
    Ok(())
}

fn verify_verifier_contract(
    request: &VerificationRequest<'_>,
    expected_binding: &xgen_domain::InstanceBinding,
    operation: QueryOperation,
) -> Result<(), VerificationPortFailure> {
    let rules = &request.definition().spec.verification;
    let intent = request.intent();
    let instance = request.instance();
    if intent.invocation.capability_id != operation.capability_id()
        || intent.invocation.contract_version != operation.contract_version()
        || instance.definition.capability_id != operation.capability_id()
        || instance.definition.contract_version != operation.contract_version()
        || intent.invocation.instance_id != instance.instance_id
        || instance.binding != *expected_binding
        || !supports_instance_features(&instance.features)
        || intent.effect_class != EffectClass::ReadOnly
        || intent.idempotency_key.is_some()
        || rules.len() != 2
        || rules.iter().any(|rule| {
            !rule.required
                || !matches!(
                    rule.strategy,
                    VerificationStrategy::OutputSchema | VerificationStrategy::Postcondition
                )
        })
        || rules
            .iter()
            .filter(|rule| rule.strategy == VerificationStrategy::OutputSchema)
            .count()
            != 1
        || rules
            .iter()
            .filter(|rule| rule.strategy == VerificationStrategy::Postcondition)
            .count()
            != 1
    {
        return Err(VerificationPortFailure::UnsupportedStrategy);
    }
    Ok(())
}

const fn supports_instance_features(features: &InstanceFeatures) -> bool {
    features.sync && !features.task && !features.cancellable && !features.idempotency_query
}

fn parse_arguments(
    arguments: &Value,
    root: &WorkspaceRoot,
    operation: QueryOperation,
) -> Result<ParsedArguments, AdapterPrepareFailure> {
    let object = arguments
        .as_object()
        .filter(|object| object.len() == operation.argument_count())
        .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
    let path = parse_canonical(&root.workspace_id, path)
        .map_err(|_| AdapterPrepareFailure::InvalidMaterial)?;
    let query = if operation == QueryOperation::SearchText {
        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|query| {
                !query.is_empty()
                    && query.len() <= MAX_SEARCH_QUERY_BYTES
                    && query.chars().count() <= MAX_SEARCH_QUERY_UNICODE_SCALARS
                    && !query.chars().any(char::is_control)
            })
            .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
        Some(query.to_owned())
    } else {
        None
    };
    let lines = if operation == QueryOperation::ReadTextLines {
        let start_line = object
            .get("startLine")
            .and_then(Value::as_u64)
            .filter(|line| *line >= 1)
            .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
        let line_count = object
            .get("lineCount")
            .and_then(Value::as_u64)
            .filter(|count| (1..=MAX_READ_TEXT_LINES_COUNT).contains(count))
            .ok_or(AdapterPrepareFailure::InvalidMaterial)?;
        Some(LineRange {
            start_line,
            line_count,
        })
    } else {
        None
    };
    Ok(ParsedArguments { path, query, lines })
}

fn list_directory(root: &Dir, path: &RelativePath) -> Result<ObservedOutput, QueryFailure> {
    let directory = open_directory(root, path)?;
    let mut entries = Vec::new();
    let mut incomplete = false;
    let mut scanned = 0_usize;
    for candidate in directory.entries().map_err(|_| QueryFailure::Read)? {
        scanned = scanned.checked_add(1).ok_or(QueryFailure::TooManyEntries)?;
        if scanned > MAX_DIRECTORY_SCAN_ENTRIES {
            return Err(QueryFailure::TooManyEntries);
        }
        let candidate = candidate.map_err(|_| QueryFailure::Read)?;
        let Ok(name) = candidate.file_name().into_string() else {
            incomplete = true;
            continue;
        };
        let Ok(child) = path.join(&name) else {
            incomplete = true;
            continue;
        };
        let file_type = candidate.file_type().map_err(|_| QueryFailure::Read)?;
        let (kind, size_bytes) = if file_type.is_symlink() {
            ("other", Value::Null)
        } else if file_type.is_dir() {
            if open_directory(root, &child).is_ok() {
                ("directory", Value::Null)
            } else {
                incomplete = true;
                ("other", Value::Null)
            }
        } else if file_type.is_file() {
            if let Ok(metadata) = open_regular_file(root, &child)
                .and_then(|file| file.metadata().map_err(|_| QueryFailure::Open))
            {
                ("file", json!(metadata.len()))
            } else {
                incomplete = true;
                ("other", Value::Null)
            }
        } else {
            ("other", Value::Null)
        };
        entries.push(json!({
            "name": name,
            "path": child.display(),
            "kind": kind,
            "sizeBytes": size_bytes,
        }));
    }
    entries.sort_by(|left, right| {
        left["path"]
            .as_str()
            .expect("constructed path is a string")
            .cmp(
                right["path"]
                    .as_str()
                    .expect("constructed path is a string"),
            )
    });
    if entries.len() > MAX_LIST_DIRECTORY_ENTRIES {
        entries.truncate(MAX_LIST_DIRECTORY_ENTRIES);
        incomplete = true;
    }
    bounded_collection_output("entries", entries, incomplete)
}

fn stat_path(root: &Dir, path: &RelativePath) -> Result<ObservedOutput, QueryFailure> {
    if path.components().is_empty() {
        open_directory(root, path)?;
        return Ok(ObservedOutput {
            payload: json!({"kind": "directory", "sizeBytes": null}),
        });
    }
    if let Ok(file) = open_regular_file(root, path) {
        let metadata = file.metadata().map_err(|_| QueryFailure::Open)?;
        return Ok(ObservedOutput {
            payload: json!({"kind": "file", "sizeBytes": metadata.len()}),
        });
    }
    open_directory(root, path)?;
    Ok(ObservedOutput {
        payload: json!({"kind": "directory", "sizeBytes": null}),
    })
}

fn search_text(
    root: &Dir,
    path: &RelativePath,
    query: &str,
) -> Result<ObservedOutput, QueryFailure> {
    open_directory(root, path)?;
    let mut state = SearchState {
        matches: Vec::new(),
        visited_entries: 0,
        scanned_bytes: 0,
        truncated: false,
        exhausted: false,
    };
    search_directory(root, path, query, &mut state)?;
    state.matches.sort_by(|left, right| {
        let left = search_match_key(left);
        let right = search_match_key(right);
        left.cmp(&right)
    });
    bounded_collection_output("matches", state.matches, state.truncated)
}

fn read_text_lines(
    root: &Dir,
    path: &RelativePath,
    range: LineRange,
) -> Result<ObservedOutput, QueryFailure> {
    let mut file = open_regular_file(root, path)?;
    let metadata = file.metadata().map_err(|_| QueryFailure::Open)?;
    if metadata.len() > u64::try_from(MAX_READ_TEXT_LINES_FILE_BYTES).expect("limit fits u64") {
        return Err(QueryFailure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_READ_TEXT_LINES_FILE_BYTES)
            .min(MAX_READ_TEXT_LINES_FILE_BYTES)
            .saturating_add(1),
    );
    file.by_ref()
        .take(u64::try_from(MAX_READ_TEXT_LINES_FILE_BYTES + 1).expect("read limit fits u64"))
        .read_to_end(&mut bytes)
        .map_err(|_| QueryFailure::Read)?;
    if bytes.len() > MAX_READ_TEXT_LINES_FILE_BYTES {
        return Err(QueryFailure::TooLarge);
    }
    let file_digest = crate::read_text::sha256_digest(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| QueryFailure::Decode)?;
    line_range_output(&content, &file_digest, range)
}

/// Select whole lines, each with its original terminator, so consecutive ranges concatenate to
/// the file. Selection stops before the JSON-escaped content would exceed its byte limit; a first
/// line that alone exceeds it is cut at a character boundary. Either case sets `truncated`.
fn line_range_output(
    content: &str,
    file_digest: &str,
    range: LineRange,
) -> Result<ObservedOutput, QueryFailure> {
    let total_lines =
        u64::try_from(content.split_inclusive('\n').count()).map_err(|_| QueryFailure::Limit)?;
    let skip = usize::try_from(range.start_line - 1).unwrap_or(usize::MAX);
    let take = usize::try_from(range.line_count).map_err(|_| QueryFailure::Limit)?;
    let mut selected = String::new();
    let mut selected_size = 0_usize;
    let mut end_line = range.start_line - 1;
    let mut truncated = false;
    for line in content.split_inclusive('\n').skip(skip).take(take) {
        let line_size = json_escaped_size(line);
        if selected_size + line_size <= MAX_READ_TEXT_LINES_CONTENT_BYTES {
            selected.push_str(line);
            selected_size += line_size;
            end_line += 1;
            continue;
        }
        truncated = true;
        if selected.is_empty() {
            let mut cut = 0;
            for (index, character) in line.char_indices() {
                let size = json_escaped_char_size(character);
                if selected_size + size > MAX_READ_TEXT_LINES_CONTENT_BYTES {
                    break;
                }
                selected_size += size;
                cut = index + character.len_utf8();
            }
            selected.push_str(&line[..cut]);
            end_line += 1;
        }
        break;
    }
    Ok(ObservedOutput {
        payload: json!({
            "content": selected,
            "startLine": range.start_line,
            "endLine": end_line,
            "totalLines": total_lines,
            "truncated": truncated,
            "fileDigest": file_digest,
        }),
    })
}

fn json_escaped_size(text: &str) -> usize {
    text.chars().map(json_escaped_char_size).sum()
}

/// Size of one character inside an RFC 8785 JSON string.
const fn json_escaped_char_size(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '\u{0}'..='\u{1f}' => 6,
        _ => character.len_utf8(),
    }
}

fn search_match_key(candidate: &Value) -> (&str, u64, u64) {
    (
        candidate["path"]
            .as_str()
            .expect("constructed search path is a string"),
        candidate["line"]
            .as_u64()
            .expect("constructed search line is an unsigned integer"),
        candidate["column"]
            .as_u64()
            .expect("constructed search column is an unsigned integer"),
    )
}

struct SearchState {
    matches: Vec<Value>,
    visited_entries: usize,
    scanned_bytes: usize,
    truncated: bool,
    exhausted: bool,
}

fn search_directory(
    root: &Dir,
    path: &RelativePath,
    query: &str,
    state: &mut SearchState,
) -> Result<(), QueryFailure> {
    if state.exhausted
        || state.matches.len() == MAX_SEARCH_MATCHES
        || state.visited_entries == MAX_SEARCH_VISITED_ENTRIES
    {
        state.truncated = true;
        return Ok(());
    }
    let directory = open_directory(root, path)?;
    let mut names = Vec::new();
    let mut scanned = 0_usize;
    for candidate in directory.entries().map_err(|_| QueryFailure::Read)? {
        if state.visited_entries == MAX_SEARCH_VISITED_ENTRIES {
            state.truncated = true;
            break;
        }
        state.visited_entries += 1;
        scanned = scanned.checked_add(1).ok_or(QueryFailure::TooManyEntries)?;
        if scanned > MAX_DIRECTORY_SCAN_ENTRIES {
            return Err(QueryFailure::TooManyEntries);
        }
        let candidate = candidate.map_err(|_| QueryFailure::Read)?;
        let Ok(name) = candidate.file_name().into_string() else {
            state.truncated = true;
            continue;
        };
        if path.join(&name).is_err() {
            state.truncated = true;
            continue;
        }
        names.push(name);
    }
    names.sort();

    for name in names {
        if state.exhausted || state.matches.len() == MAX_SEARCH_MATCHES {
            state.truncated = true;
            break;
        }
        let child = path.join(&name).map_err(|_| QueryFailure::Read)?;
        if matches!(name.as_str(), ".git" | ".hg" | ".svn") {
            state.truncated = true;
            continue;
        }
        if open_directory(root, &child).is_ok() {
            search_directory(root, &child, query, state)?;
            continue;
        }
        search_regular_file(root, &child, query, state)?;
        if state.exhausted {
            break;
        }
    }
    Ok(())
}

fn search_regular_file(
    root: &Dir,
    path: &RelativePath,
    query: &str,
    state: &mut SearchState,
) -> Result<(), QueryFailure> {
    let Ok(mut file) = open_regular_file(root, path) else {
        state.truncated = true;
        return Ok(());
    };
    let metadata = file.metadata().map_err(|_| QueryFailure::Open)?;
    if metadata.len() > u64::try_from(MAX_SEARCH_FILE_BYTES).expect("limit fits u64") {
        state.truncated = true;
        return Ok(());
    }
    let remaining = MAX_SEARCH_TOTAL_BYTES.saturating_sub(state.scanned_bytes);
    if remaining == 0
        || metadata.len() > u64::try_from(remaining).expect("aggregate limit fits u64")
    {
        state.truncated = true;
        state.exhausted = true;
        return Ok(());
    }
    let read_limit = remaining.min(MAX_SEARCH_FILE_BYTES.saturating_add(1));
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_SEARCH_FILE_BYTES)
            .min(MAX_SEARCH_FILE_BYTES)
            .saturating_add(1),
    );
    file.by_ref()
        .take(u64::try_from(read_limit).expect("read limit fits u64"))
        .read_to_end(&mut bytes)
        .map_err(|_| QueryFailure::Read)?;
    state.scanned_bytes = state
        .scanned_bytes
        .checked_add(bytes.len())
        .ok_or(QueryFailure::Limit)?;
    if state.scanned_bytes == MAX_SEARCH_TOTAL_BYTES {
        state.truncated = true;
        state.exhausted = true;
    }
    if bytes.len() > MAX_SEARCH_FILE_BYTES {
        state.truncated = true;
        return Ok(());
    }
    let Ok(content) = String::from_utf8(bytes) else {
        state.truncated = true;
        return Ok(());
    };
    collect_matches(path, &content, query, state)
}

fn collect_matches(
    path: &RelativePath,
    content: &str,
    query: &str,
    state: &mut SearchState,
) -> Result<(), QueryFailure> {
    for (line_index, line) in content.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        for (byte_index, matched) in line.match_indices(query) {
            if state.matches.len() == MAX_SEARCH_MATCHES {
                state.truncated = true;
                return Ok(());
            }
            let line_number = u64::try_from(line_index + 1).map_err(|_| QueryFailure::Limit)?;
            let column = u64::try_from(line[..byte_index].chars().count() + 1)
                .map_err(|_| QueryFailure::Limit)?;
            state.matches.push(json!({
                "path": path.display(),
                "line": line_number,
                "column": column,
                "preview": bounded_preview(line, byte_index, byte_index + matched.len()),
            }));
        }
    }
    Ok(())
}

fn bounded_preview(line: &str, match_start: usize, match_end: usize) -> String {
    if line.len() <= MAX_SEARCH_PREVIEW_BYTES {
        return line.to_owned();
    }
    let match_len = match_end.saturating_sub(match_start);
    let margin = MAX_SEARCH_PREVIEW_BYTES.saturating_sub(match_len) / 2;
    let mut start = match_start.saturating_sub(margin);
    while start > 0 && !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = start
        .saturating_add(MAX_SEARCH_PREVIEW_BYTES)
        .min(line.len());
    while end > match_end && !line.is_char_boundary(end) {
        end -= 1;
    }
    if end < match_end {
        end = match_end;
        while start > 0 && end.saturating_sub(start) > MAX_SEARCH_PREVIEW_BYTES {
            start += 1;
            while start < match_start && !line.is_char_boundary(start) {
                start += 1;
            }
        }
    }
    line[start..end].to_owned()
}

fn open_directory(root: &Dir, path: &RelativePath) -> Result<Dir, QueryFailure> {
    let mut directory = root.try_clone().map_err(|_| QueryFailure::Open)?;
    let last = path.components().len().saturating_sub(1);
    for (index, component) in path.components().iter().enumerate() {
        directory = directory.open_dir_nofollow(component).map_err(|error| {
            match classify_open_error(&directory, component, &error) {
                // A non-directory final component exists but has the wrong kind; an earlier one
                // means the requested path does not exist.
                OpenErrorClass::NotADirectory if index == last => QueryFailure::NotADirectory,
                OpenErrorClass::Missing | OpenErrorClass::NotADirectory => QueryFailure::NotFound,
                OpenErrorClass::Directory | OpenErrorClass::Refused => QueryFailure::Open,
            }
        })?;
        if is_windows_reparse_point(&directory.dir_metadata().map_err(|_| QueryFailure::Open)?) {
            return Err(QueryFailure::Open);
        }
    }
    Ok(directory)
}

fn open_regular_file(root: &Dir, path: &RelativePath) -> Result<File, QueryFailure> {
    // The workspace root itself is a directory.
    let (leaf, parents) = path
        .components()
        .split_last()
        .ok_or(QueryFailure::NotAFile)?;
    let parent = RelativePath::from_components(parents.to_vec());
    let directory = open_directory(root, &parent).map_err(|failure| match failure {
        QueryFailure::NotADirectory => QueryFailure::NotFound,
        other => other,
    })?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = directory.open_with(leaf, &options).map_err(|error| {
        match classify_open_error(&directory, leaf, &error) {
            OpenErrorClass::Missing | OpenErrorClass::NotADirectory => QueryFailure::NotFound,
            OpenErrorClass::Directory => QueryFailure::NotAFile,
            OpenErrorClass::Refused => QueryFailure::Open,
        }
    })?;
    let metadata = file.metadata().map_err(|_| QueryFailure::Open)?;
    if is_windows_reparse_point(&metadata) {
        return Err(QueryFailure::Open);
    }
    if !metadata.is_file() {
        return Err(QueryFailure::NotAFile);
    }
    Ok(file)
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_windows_reparse_point(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

fn inspect_output(
    output: &Value,
    outcome_evidence_digest: &str,
    workspace_id: &crate::WorkspaceId,
    operation: QueryOperation,
) -> Result<InspectedOutput, VerificationPortFailure> {
    if serde_jcs::to_vec(output)
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?
        .len()
        > MAX_QUERY_OUTPUT_CANONICAL_BYTES
    {
        return Err(VerificationPortFailure::ResponseUnverifiable);
    }
    let object = output
        .as_object()
        .filter(|object| object.len() == operation.output_field_count())
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let claimed = object
        .get("digest")
        .and_then(Value::as_str)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    AdapterEvidenceDigest::new(claimed.to_owned())
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
    validate_output_shape(object, workspace_id, operation)?;
    let mut payload = object.clone();
    payload.remove("digest");
    let canonical = serde_jcs::to_vec(&Value::Object(payload))
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
    let digest = sha256_digest(&canonical);
    Ok(InspectedOutput {
        postcondition: claimed == digest && outcome_evidence_digest == digest,
        byte_size: u64::try_from(canonical.len())
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?,
        digest,
    })
}

fn validate_output_shape(
    object: &Map<String, Value>,
    workspace_id: &crate::WorkspaceId,
    operation: QueryOperation,
) -> Result<(), VerificationPortFailure> {
    match operation {
        QueryOperation::ListDirectory => validate_list_output(object, workspace_id),
        QueryOperation::Stat => validate_stat_output(object),
        QueryOperation::SearchText => validate_search_output(object, workspace_id),
        QueryOperation::ReadTextLines => validate_lines_output(object),
    }
}

fn validate_lines_output(object: &Map<String, Value>) -> Result<(), VerificationPortFailure> {
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| json_escaped_size(content) <= MAX_READ_TEXT_LINES_CONTENT_BYTES)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let start_line = object
        .get("startLine")
        .and_then(Value::as_u64)
        .filter(|line| *line >= 1)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let end_line = object
        .get("endLine")
        .and_then(Value::as_u64)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let total_lines = object
        .get("totalLines")
        .and_then(Value::as_u64)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    object
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    object
        .get("fileDigest")
        .and_then(Value::as_str)
        .filter(|digest| crate::read_text::is_canonical_digest(digest))
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let returned = u64::try_from(content.split_inclusive('\n').count())
        .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
    if returned > MAX_READ_TEXT_LINES_COUNT
        || end_line.checked_add(1) != start_line.checked_add(returned)
        || (returned > 0 && end_line > total_lines)
    {
        return Err(VerificationPortFailure::ResponseUnverifiable);
    }
    Ok(())
}

fn validate_list_output(
    object: &Map<String, Value>,
    workspace_id: &crate::WorkspaceId,
) -> Result<(), VerificationPortFailure> {
    object
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let entries = object
        .get("entries")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() <= MAX_LIST_DIRECTORY_ENTRIES)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let mut previous = None;
    for entry in entries {
        let entry = entry
            .as_object()
            .filter(|entry| entry.len() == 4)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let canonical = canonical_resource(workspace_id, path)
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        let parsed = parse_canonical(workspace_id, &canonical)
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        if parsed.display() != path
            || parsed.components().last().map(String::as_str) != Some(name)
            || previous.is_some_and(|previous: &str| previous >= path)
        {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
        previous = Some(path);
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .filter(|kind| matches!(*kind, "file" | "directory" | "other"))
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let size = entry
            .get("sizeBytes")
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        if (kind == "file" && size.as_u64().is_none()) || (kind != "file" && !size.is_null()) {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
    }
    Ok(())
}

fn validate_stat_output(object: &Map<String, Value>) -> Result<(), VerificationPortFailure> {
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "file" | "directory"))
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let size = object
        .get("sizeBytes")
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    if (kind == "file" && size.as_u64().is_none()) || (kind == "directory" && !size.is_null()) {
        return Err(VerificationPortFailure::ResponseUnverifiable);
    }
    Ok(())
}

fn validate_search_output(
    object: &Map<String, Value>,
    workspace_id: &crate::WorkspaceId,
) -> Result<(), VerificationPortFailure> {
    object
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let matches = object
        .get("matches")
        .and_then(Value::as_array)
        .filter(|matches| matches.len() <= MAX_SEARCH_MATCHES)
        .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
    let mut previous: Option<(String, u64, u64)> = None;
    for candidate in matches {
        let candidate = candidate
            .as_object()
            .filter(|candidate| candidate.len() == 4)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let path = candidate
            .get("path")
            .and_then(Value::as_str)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let canonical = canonical_resource(workspace_id, path)
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        let parsed = parse_canonical(workspace_id, &canonical)
            .map_err(|_| VerificationPortFailure::ResponseUnverifiable)?;
        if parsed.components().is_empty() || parsed.display() != path {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
        let line = candidate
            .get("line")
            .and_then(Value::as_u64)
            .filter(|line| *line > 0)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let column = candidate
            .get("column")
            .and_then(Value::as_u64)
            .filter(|column| *column > 0)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        candidate
            .get("preview")
            .and_then(Value::as_str)
            .filter(|preview| preview.len() <= MAX_SEARCH_PREVIEW_BYTES)
            .ok_or(VerificationPortFailure::ResponseUnverifiable)?;
        let key = (path.to_owned(), line, column);
        if previous.as_ref().is_some_and(|previous| previous >= &key) {
            return Err(VerificationPortFailure::ResponseUnverifiable);
        }
        previous = Some(key);
    }
    Ok(())
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{encoded}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::{WorkspaceId, WorkspaceRoot};

    struct Fixture {
        root: WorkspaceRoot,
        directory: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempdir().expect("temporary workspace should exist");
            let root = WorkspaceRoot::open_ambient(
                directory.path(),
                WorkspaceId::new("fixture").expect("workspace ID should validate"),
            )
            .expect("workspace should open");
            Self { root, directory }
        }

        fn relative(&self, path: &str) -> RelativePath {
            let canonical = canonical_resource(&self.root.workspace_id, path)
                .expect("fixture path should resolve");
            parse_canonical(&self.root.workspace_id, &canonical).expect("fixture path should parse")
        }
    }

    #[test]
    fn list_directory_is_sorted_bounded_and_root_relative() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("z.txt"), "z").unwrap();
        fs::create_dir(fixture.directory.path().join("src")).unwrap();
        fs::write(fixture.directory.path().join("a.txt"), "alpha").unwrap();

        let output = list_directory(&fixture.root.directory, &fixture.relative("."))
            .unwrap()
            .finish()
            .unwrap()
            .output;

        assert_eq!(output["entries"][0]["path"], "a.txt");
        assert_eq!(output["entries"][0]["sizeBytes"], 5);
        assert_eq!(output["entries"][1]["path"], "src");
        assert_eq!(output["entries"][1]["kind"], "directory");
        assert_eq!(output["entries"][2]["path"], "z.txt");
        assert_eq!(output["truncated"], false);
        assert!(output["digest"].as_str().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn stat_reports_only_file_size_or_directory_kind() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("file.txt"), "hello").unwrap();
        fs::create_dir(fixture.directory.path().join("dir")).unwrap();

        let file = stat_path(&fixture.root.directory, &fixture.relative("file.txt"))
            .unwrap()
            .finish()
            .unwrap()
            .output;
        let directory = stat_path(&fixture.root.directory, &fixture.relative("dir"))
            .unwrap()
            .finish()
            .unwrap()
            .output;
        let root = stat_path(&fixture.root.directory, &fixture.relative("."))
            .unwrap()
            .finish()
            .unwrap()
            .output;

        assert_eq!(file["kind"], "file");
        assert_eq!(file["sizeBytes"], 5);
        assert_eq!(directory["kind"], "directory");
        assert_eq!(directory["sizeBytes"], Value::Null);
        assert_eq!(root["kind"], "directory");
    }

    #[test]
    fn search_is_recursive_literal_utf8_and_deterministic() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.directory.path().join("src/nested")).unwrap();
        fs::write(
            fixture.directory.path().join("src/lib.rs"),
            "zero\nneedle here\nneedle again",
        )
        .unwrap();
        fs::write(
            fixture.directory.path().join("src/nested/mod.rs"),
            "앞 needle 뒤",
        )
        .unwrap();
        fs::write(
            fixture.directory.path().join("src/binary.bin"),
            [0xff, 0xfe],
        )
        .unwrap();

        let output = search_text(&fixture.root.directory, &fixture.relative("src"), "needle")
            .unwrap()
            .finish()
            .unwrap()
            .output;

        assert_eq!(output["matches"].as_array().unwrap().len(), 3);
        assert_eq!(output["matches"][0]["path"], "src/lib.rs");
        assert_eq!(output["matches"][0]["line"], 2);
        assert_eq!(output["matches"][0]["column"], 1);
        assert_eq!(output["matches"][2]["path"], "src/nested/mod.rs");
        assert_eq!(output["matches"][2]["column"], 3);
        assert_eq!(output["truncated"], true);
    }

    #[test]
    fn search_order_is_globally_lexical_across_path_prefixes() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.directory.path().join("a")).unwrap();
        fs::write(fixture.directory.path().join("a/z.txt"), "needle").unwrap();
        fs::write(fixture.directory.path().join("a.txt"), "needle").unwrap();

        let finished = search_text(&fixture.root.directory, &fixture.relative("."), "needle")
            .unwrap()
            .finish()
            .unwrap();

        assert_eq!(finished.output["matches"][0]["path"], "a.txt");
        assert_eq!(finished.output["matches"][1]["path"], "a/z.txt");
        assert!(
            inspect_output(
                &finished.output,
                &finished.digest,
                &fixture.root.workspace_id,
                QueryOperation::SearchText,
            )
            .unwrap()
            .postcondition
        );
    }

    #[test]
    fn collection_output_is_truncated_before_the_canonical_byte_limit() {
        let candidates = (0..MAX_LIST_DIRECTORY_ENTRIES)
            .map(|index| {
                json!({
                    "name": format!("entry-{index:04}"),
                    "path": format!("{}/entry-{index:04}", "x".repeat(4_000)),
                    "kind": "file",
                    "sizeBytes": 1,
                })
            })
            .collect();

        let finished = bounded_collection_output("entries", candidates, false)
            .unwrap()
            .finish()
            .unwrap();

        assert_eq!(finished.output["truncated"], true);
        assert!(finished.output["entries"].as_array().unwrap().len() < 512);
        assert!(
            serde_jcs::to_vec(&finished.output).unwrap().len() <= MAX_QUERY_OUTPUT_CANONICAL_BYTES
        );
    }

    #[test]
    fn aggregate_search_budget_counts_bytes_and_stops_globally() {
        let fixture = Fixture::new();
        let content = vec![b'x'; MAX_SEARCH_FILE_BYTES];
        for index in 0..=MAX_SEARCH_TOTAL_BYTES / MAX_SEARCH_FILE_BYTES {
            fs::write(
                fixture
                    .directory
                    .path()
                    .join(format!("file-{index:04}.txt")),
                &content,
            )
            .unwrap();
        }
        let mut state = SearchState {
            matches: Vec::new(),
            visited_entries: 0,
            scanned_bytes: 0,
            truncated: false,
            exhausted: false,
        };

        search_directory(
            &fixture.root.directory,
            &fixture.relative("."),
            "not-present",
            &mut state,
        )
        .unwrap();

        assert_eq!(state.scanned_bytes, MAX_SEARCH_TOTAL_BYTES);
        assert!(state.exhausted);
        assert!(state.truncated);
    }

    #[test]
    fn recursive_entry_budget_is_charged_while_directory_entries_are_collected() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("a.txt"), "first").unwrap();
        fs::write(fixture.directory.path().join("b.txt"), "second").unwrap();
        let mut state = SearchState {
            matches: Vec::new(),
            visited_entries: MAX_SEARCH_VISITED_ENTRIES - 1,
            scanned_bytes: 0,
            truncated: false,
            exhausted: false,
        };

        search_directory(
            &fixture.root.directory,
            &fixture.relative("."),
            "absent",
            &mut state,
        )
        .unwrap();

        assert_eq!(state.visited_entries, MAX_SEARCH_VISITED_ENTRIES);
        assert!(state.truncated);
        assert!(state.scanned_bytes <= "second".len());
    }

    fn lines(start_line: u64, line_count: u64) -> LineRange {
        LineRange {
            start_line,
            line_count,
        }
    }

    #[test]
    fn line_ranges_keep_terminators_and_concatenate_to_the_file() {
        let fixture = Fixture::new();
        let text = "one\r\ntwo\n\nfour\nfive";
        fs::write(fixture.directory.path().join("file.txt"), text).unwrap();
        let path = fixture.relative("file.txt");

        let first = read_text_lines(&fixture.root.directory, &path, lines(1, 2))
            .unwrap()
            .finish()
            .unwrap()
            .output;
        let rest = read_text_lines(&fixture.root.directory, &path, lines(3, 10))
            .unwrap()
            .finish()
            .unwrap()
            .output;

        assert_eq!(first["content"], "one\r\ntwo\n");
        assert_eq!(first["startLine"], 1);
        assert_eq!(first["endLine"], 2);
        assert_eq!(first["totalLines"], 5);
        assert_eq!(first["truncated"], false);
        let whole_file = crate::read_text::sha256_digest(text.as_bytes());
        assert_eq!(first["fileDigest"], whole_file);
        assert_eq!(rest["fileDigest"], whole_file);
        assert_eq!(rest["content"], "\nfour\nfive");
        assert_eq!(rest["endLine"], 5);
        assert_eq!(rest["truncated"], false);
        assert_eq!(
            format!(
                "{}{}",
                first["content"].as_str().unwrap(),
                rest["content"].as_str().unwrap()
            ),
            text
        );
    }

    #[test]
    fn a_range_past_the_end_is_empty_and_reports_the_line_count() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("file.txt"), "a\nb\n").unwrap();
        fs::write(fixture.directory.path().join("empty.txt"), "").unwrap();

        let past = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("file.txt"),
            lines(9, 5),
        )
        .unwrap()
        .finish()
        .unwrap();
        let empty = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("empty.txt"),
            lines(1, 5),
        )
        .unwrap()
        .finish()
        .unwrap()
        .output;

        assert_eq!(past.output["content"], "");
        assert_eq!(past.output["endLine"], 8);
        assert_eq!(past.output["totalLines"], 2);
        assert!(
            inspect_output(
                &past.output,
                &past.digest,
                &fixture.root.workspace_id,
                QueryOperation::ReadTextLines,
            )
            .unwrap()
            .postcondition
        );
        assert_eq!(empty["content"], "");
        assert_eq!(empty["endLine"], 0);
        assert_eq!(empty["totalLines"], 0);
    }

    #[test]
    fn line_ranges_stop_at_the_escaped_content_limit() {
        let fixture = Fixture::new();
        // Each line escapes to 1_024 bytes: 1_020 plain characters, a quote, and a newline.
        let line = format!("{}\"\n", "x".repeat(1_020));
        fs::write(fixture.directory.path().join("wide.txt"), line.repeat(100)).unwrap();
        let control_line = "\u{1}".repeat(MAX_READ_TEXT_LINES_CONTENT_BYTES);
        fs::write(
            fixture.directory.path().join("long.txt"),
            format!("{control_line}\nnext\n"),
        )
        .unwrap();

        let wide = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("wide.txt"),
            lines(1, 100),
        )
        .unwrap()
        .finish()
        .unwrap();
        let long = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("long.txt"),
            lines(1, 2),
        )
        .unwrap()
        .finish()
        .unwrap();

        assert_eq!(wide.output["endLine"], 64);
        assert_eq!(wide.output["totalLines"], 100);
        assert_eq!(wide.output["truncated"], true);
        assert_eq!(long.output["endLine"], 1);
        assert_eq!(long.output["truncated"], true);
        assert_eq!(
            long.output["content"].as_str().unwrap().chars().count(),
            MAX_READ_TEXT_LINES_CONTENT_BYTES / 6
        );
        for finished in [&wide, &long] {
            assert!(
                serde_jcs::to_vec(&finished.output).unwrap().len()
                    <= MAX_QUERY_OUTPUT_CANONICAL_BYTES
            );
            assert!(
                inspect_output(
                    &finished.output,
                    &finished.digest,
                    &fixture.root.workspace_id,
                    QueryOperation::ReadTextLines,
                )
                .unwrap()
                .postcondition
            );
        }
    }

    #[test]
    fn line_ranges_read_files_past_the_whole_file_limit_but_fail_closed_beyond_the_scan_limit() {
        let fixture = Fixture::new();
        let large = "line\n".repeat(20_000);
        fs::write(fixture.directory.path().join("large.txt"), &large).unwrap();
        fs::write(
            fixture.directory.path().join("huge.txt"),
            vec![b'x'; MAX_READ_TEXT_LINES_FILE_BYTES + 1],
        )
        .unwrap();
        fs::write(fixture.directory.path().join("binary.bin"), [0xff, 0xfe]).unwrap();

        let tail = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("large.txt"),
            lines(19_999, 5),
        )
        .unwrap()
        .finish()
        .unwrap()
        .output;

        assert!(large.len() > crate::MAX_READ_TEXT_BYTES);
        assert_eq!(tail["content"], "line\nline\n");
        assert_eq!(tail["totalLines"], 20_000);
        assert!(matches!(
            read_text_lines(
                &fixture.root.directory,
                &fixture.relative("huge.txt"),
                lines(1, 1)
            ),
            Err(QueryFailure::TooLarge)
        ));
        assert!(matches!(
            read_text_lines(
                &fixture.root.directory,
                &fixture.relative("binary.bin"),
                lines(1, 1)
            ),
            Err(QueryFailure::Decode)
        ));
        assert!(matches!(
            read_text_lines(&fixture.root.directory, &fixture.relative("."), lines(1, 1)),
            Err(QueryFailure::NotAFile)
        ));
    }

    #[test]
    fn line_range_parser_requires_a_bounded_one_based_range() {
        let fixture = Fixture::new();
        let parsed = parse_arguments(
            &json!({"path": "workspace:fixture/a.txt", "startLine": 3, "lineCount": 2000}),
            &fixture.root,
            QueryOperation::ReadTextLines,
        )
        .unwrap();
        assert_eq!(parsed.lines, Some(lines(3, 2_000)));
        for arguments in [
            json!({"path": "workspace:fixture/a.txt"}),
            json!({"path": "workspace:fixture/a.txt", "startLine": 0, "lineCount": 1}),
            json!({"path": "workspace:fixture/a.txt", "startLine": 1, "lineCount": 0}),
            json!({"path": "workspace:fixture/a.txt", "startLine": 1, "lineCount": 2001}),
            json!({"path": "workspace:fixture/a.txt", "startLine": 1.5, "lineCount": 1}),
            json!({"path": "workspace:fixture/a.txt", "startLine": 1, "lineCount": 1, "x": 1}),
        ] {
            assert_eq!(
                parse_arguments(&arguments, &fixture.root, QueryOperation::ReadTextLines)
                    .map(|_| ()),
                Err(AdapterPrepareFailure::InvalidMaterial)
            );
        }
    }

    #[test]
    fn line_range_inspection_rejects_inconsistent_line_numbers() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("file.txt"), "a\nb\nc\n").unwrap();
        let finished = read_text_lines(
            &fixture.root.directory,
            &fixture.relative("file.txt"),
            lines(2, 1),
        )
        .unwrap()
        .finish()
        .unwrap();
        for (field, value) in [
            ("endLine", json!(3)),
            ("startLine", json!(0)),
            ("totalLines", json!(1)),
            ("content", json!("b\nc\n")),
        ] {
            let mut tampered = finished.output.clone();
            tampered[field] = value;
            assert!(
                inspect_output(
                    &tampered,
                    &finished.digest,
                    &fixture.root.workspace_id,
                    QueryOperation::ReadTextLines,
                )
                .is_err(),
                "{field} tampering must fail closed"
            );
        }
    }

    #[test]
    fn failures_name_missing_paths_and_wrong_kinds_without_values() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("file.txt"), "text").unwrap();
        fs::create_dir(fixture.directory.path().join("dir")).unwrap();
        let root = &fixture.root.directory;

        for (observed, expected) in [
            (
                list_directory(root, &fixture.relative("missing")).err(),
                StepFailureCode::NotFound,
            ),
            (
                list_directory(root, &fixture.relative("file.txt")).err(),
                StepFailureCode::NotADirectory,
            ),
            (
                list_directory(root, &fixture.relative("file.txt/child")).err(),
                StepFailureCode::NotFound,
            ),
            (
                stat_path(root, &fixture.relative("missing.txt")).err(),
                StepFailureCode::NotFound,
            ),
            (
                read_text_lines(root, &fixture.relative("missing.txt"), lines(1, 1)).err(),
                StepFailureCode::NotFound,
            ),
            (
                read_text_lines(root, &fixture.relative("file.txt/child"), lines(1, 1)).err(),
                StepFailureCode::NotFound,
            ),
            (
                read_text_lines(root, &fixture.relative("dir"), lines(1, 1)).err(),
                StepFailureCode::NotAFile,
            ),
        ] {
            assert_eq!(
                observed.map(QueryFailure::failure_code),
                Some(expected),
                "{expected:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn queries_never_follow_workspace_symlinks() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "OUTSIDE-SECRET").unwrap();
        symlink(outside.path(), fixture.directory.path().join("linked")).unwrap();

        assert!(list_directory(&fixture.root.directory, &fixture.relative("linked")).is_err());
        assert!(stat_path(&fixture.root.directory, &fixture.relative("linked")).is_err());
        assert!(
            search_text(
                &fixture.root.directory,
                &fixture.relative("linked"),
                "SECRET"
            )
            .is_err()
        );
        assert!(
            read_text_lines(
                &fixture.root.directory,
                &fixture.relative("linked/secret.txt"),
                lines(1, 1)
            )
            .is_err()
        );
        let root = list_directory(&fixture.root.directory, &fixture.relative("."))
            .unwrap()
            .finish()
            .unwrap()
            .output;
        assert_eq!(root["entries"][0]["kind"], "other");
    }

    #[cfg(windows)]
    #[test]
    fn queries_never_follow_workspace_junctions() {
        use std::process::Command;

        let fixture = Fixture::new();
        let outside = tempdir().expect("outside directory should exist");
        fs::write(outside.path().join("secret.txt"), "OUTSIDE-JUNCTION-SECRET")
            .expect("outside fixture should write");
        let junction = fixture.directory.path().join("junction");
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .status()
            .expect("junction command should run");
        assert!(status.success(), "junction fixture must be available in CI");

        assert!(list_directory(&fixture.root.directory, &fixture.relative("junction")).is_err());
        assert!(stat_path(&fixture.root.directory, &fixture.relative("junction")).is_err());
        assert!(
            search_text(
                &fixture.root.directory,
                &fixture.relative("junction"),
                "SECRET"
            )
            .is_err()
        );
        let root_search = search_text(&fixture.root.directory, &fixture.relative("."), "SECRET")
            .unwrap()
            .finish()
            .unwrap()
            .output;
        assert!(root_search["matches"].as_array().unwrap().is_empty());
        assert_eq!(root_search["truncated"], true);
        fs::remove_dir(&junction).expect("junction fixture should remove without following it");
    }

    #[test]
    fn parser_rejects_wrong_shapes_and_control_queries() {
        let fixture = Fixture::new();
        let maximum_unicode_query = "🦀".repeat(64);
        assert!(
            parse_arguments(
                &json!({"path": "workspace:fixture", "query": "needle"}),
                &fixture.root,
                QueryOperation::SearchText,
            )
            .is_ok()
        );
        assert!(
            parse_arguments(
                &json!({"path": "workspace:fixture", "query": maximum_unicode_query}),
                &fixture.root,
                QueryOperation::SearchText,
            )
            .is_ok()
        );
        for arguments in [
            json!({"path": "workspace:fixture"}),
            json!({"path": "workspace:fixture", "query": ""}),
            json!({"path": "workspace:fixture", "query": "bad\nquery"}),
            json!({"path": "workspace:fixture", "query": "a".repeat(65)}),
            json!({"path": "workspace:fixture", "query": "🦀".repeat(65)}),
            json!({"path": "workspace:other", "query": "needle"}),
        ] {
            assert_eq!(
                parse_arguments(&arguments, &fixture.root, QueryOperation::SearchText).map(|_| ()),
                Err(AdapterPrepareFailure::InvalidMaterial)
            );
        }
    }

    #[test]
    fn output_inspection_rejects_digest_or_order_tampering() {
        let fixture = Fixture::new();
        fs::write(fixture.directory.path().join("a.txt"), "a").unwrap();
        let output = list_directory(&fixture.root.directory, &fixture.relative("."))
            .unwrap()
            .finish()
            .unwrap()
            .output;
        let digest = output["digest"].as_str().unwrap().to_owned();
        assert!(
            inspect_output(
                &output,
                &digest,
                &fixture.root.workspace_id,
                QueryOperation::ListDirectory,
            )
            .unwrap()
            .postcondition
        );
        let mut tampered = output;
        tampered["entries"][0]["sizeBytes"] = json!(99);
        assert!(
            !inspect_output(
                &tampered,
                &digest,
                &fixture.root.workspace_id,
                QueryOperation::ListDirectory,
            )
            .unwrap()
            .postcondition
        );

        let canonical_path = ObservedOutput {
            payload: json!({
                "entries": [{
                    "name": "a.txt",
                    "path": "workspace:fixture/a.txt",
                    "kind": "file",
                    "sizeBytes": 1,
                }],
                "truncated": false,
            }),
        }
        .finish()
        .unwrap();
        assert!(
            inspect_output(
                &canonical_path.output,
                &canonical_path.digest,
                &fixture.root.workspace_id,
                QueryOperation::ListDirectory,
            )
            .is_err()
        );
    }
}
