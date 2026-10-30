//! Workspace changes a Run committed, rebuilt from its verified journal (ADR-0057).
//!
//! The model's final summary is a claim. This report comes from the records Core verified: each
//! Step whose Receipt succeeded, the exact arguments the user approved for it, and the adapter
//! output that the verifier checked. It never reads the workspace again.

use serde_json::Value;
use xgen_adapter_process::PROCESS_EXECUTE_CAPABILITY_ID;
use xgen_domain::ReceiptStatus;
use xgen_local_store::{RunStore, SqliteRunStore};
use xgen_workgraph::{InvocationMaterialRetention, StepStatus};

use crate::composition::PublicRunError;
use crate::material_catalog::RunMaterialCatalog;
use crate::run_layout::{RunLayout, discover_state_root};

const APPLY_PATCH: &str = "xgeny.fs/apply-patch";
const WRITE_ATOMIC: &str = "xgeny.fs/write-atomic";
const TERMINAL_START: &str = "xgen.terminal/start";
const TERMINAL_WRITE: &str = "xgen.terminal/write";

/// One committed Step that changed, or may have changed, the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedChange {
    /// Exact text edits applied to one existing file.
    Patch {
        path: String,
        changed: bool,
        edits: Vec<TextEdit>,
    },
    /// A whole file created or replaced with the given content.
    Write {
        path: String,
        created: bool,
        changed: bool,
        content: String,
    },
    /// A catalogued executable; any files it touched are not tracked.
    Execute {
        executable: String,
        args: Vec<String>,
    },
    /// Input sent to a terminal session; any files it touched are not tracked.
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub old_text: String,
    pub new_text: String,
}

/// Rebuild the workspace changes one local Run committed, in Receipt order.
///
/// # Errors
///
/// Returns [`PublicRunError::Configuration`] when the Run does not exist and
/// [`PublicRunError::Integrity`] when its journal or material records do not verify.
pub fn inspect_local_changes(run_id: &str) -> Result<Vec<CommittedChange>, PublicRunError> {
    let root = discover_state_root().map_err(|_| PublicRunError::Configuration)?;
    let layout =
        RunLayout::existing(&root, run_id).map_err(crate::composition::map_layout_existing)?;
    let store = SqliteRunStore::open_existing_read_only(layout.database_path())
        .map_err(|_| PublicRunError::Integrity)?;
    let state = store
        .load_current()
        .map_err(|_| PublicRunError::Integrity)?
        .ok_or(PublicRunError::Integrity)?;
    if state.run_id != run_id {
        return Err(PublicRunError::Integrity);
    }
    let mut catalog = None;
    let mut changes = Vec::new();
    for receipt in store
        .load_execution_receipts()
        .map_err(|_| PublicRunError::Integrity)?
    {
        let capability_id = receipt.capability.capability_id.as_str();
        if receipt.status != ReceiptStatus::Succeeded
            || ![
                APPLY_PATCH,
                WRITE_ATOMIC,
                PROCESS_EXECUTE_CAPABILITY_ID,
                TERMINAL_START,
                TERMINAL_WRITE,
            ]
            .contains(&capability_id)
        {
            continue;
        }
        let step = state
            .steps
            .get(&receipt.step_id)
            .ok_or(PublicRunError::Integrity)?;
        if step.status != StepStatus::Completed {
            continue;
        }
        if matches!(capability_id, TERMINAL_START | TERMINAL_WRITE) {
            changes.push(CommittedChange::Terminal);
            continue;
        }
        let intent = step.intent.as_ref().ok_or(PublicRunError::Integrity)?;
        let material = store
            .load_invocation_material(&intent.effect_id)
            .map_err(|_| PublicRunError::Integrity)?
            .ok_or(PublicRunError::Integrity)?;
        let InvocationMaterialRetention::ReconstructableReference(reference) = material.retention()
        else {
            return Err(PublicRunError::Integrity);
        };
        if catalog.is_none() {
            catalog = Some(
                RunMaterialCatalog::open_existing_read_only(
                    &layout.material_catalog_path(),
                    run_id,
                )
                .map_err(|_| PublicRunError::Integrity)?,
            );
        }
        let Some(catalog) = catalog.as_ref() else {
            return Err(PublicRunError::Integrity);
        };
        let arguments = if capability_id == PROCESS_EXECUTE_CAPABILITY_ID {
            catalog.process_arguments(reference, &step.step_id, material.material_digest())
        } else {
            catalog.workspace_arguments(
                reference,
                &step.step_id,
                material.material_digest(),
                capability_id,
            )
        }
        .map_err(|_| PublicRunError::Integrity)?;
        let output = store
            .load_tool_output(&intent.effect_id)
            .map_err(|_| PublicRunError::Integrity)?;
        let applied = output
            .as_ref()
            .and_then(|record| record.output().get("changed"))
            .and_then(Value::as_bool);
        changes.push(
            change_from(capability_id, &arguments, applied).ok_or(PublicRunError::Integrity)?,
        );
    }
    Ok(changes)
}

/// What one finished Step did, for the live activity log (ADR-0070).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepActivity {
    /// Short past-tense verb such as `read`, `patched`, or `ran`.
    pub verb: String,
    /// The workspace-relative path, command line, or query the Step acted on.
    pub subject: String,
    /// A short result such as `+3 -1`, `exit 1`, or `12 matches`, when one is known.
    pub detail: Option<String>,
    /// The closed failure code when the Step failed.
    pub failure: Option<String>,
}

/// Activities of the Receipts after the first `skip` ones, in Receipt order, rebuilt from the
/// verified journal like [`inspect_local_changes`]. A Step whose arguments cannot be rebuilt is
/// reported by its capability alone.
///
/// # Errors
///
/// Returns [`PublicRunError::Configuration`] when the Run does not exist and
/// [`PublicRunError::Integrity`] when its journal does not open.
pub fn inspect_step_activities(
    run_id: &str,
    skip: usize,
) -> Result<Vec<StepActivity>, PublicRunError> {
    let root = discover_state_root().map_err(|_| PublicRunError::Configuration)?;
    let layout =
        RunLayout::existing(&root, run_id).map_err(crate::composition::map_layout_existing)?;
    let store = SqliteRunStore::open_existing_read_only(layout.database_path())
        .map_err(|_| PublicRunError::Integrity)?;
    let receipts = store
        .load_execution_receipts()
        .map_err(|_| PublicRunError::Integrity)?;
    if receipts.len() <= skip {
        return Ok(Vec::new());
    }
    let state = store
        .load_current()
        .map_err(|_| PublicRunError::Integrity)?
        .ok_or(PublicRunError::Integrity)?;
    let catalog =
        RunMaterialCatalog::open_existing_read_only(&layout.material_catalog_path(), run_id).ok();
    let mut activities = Vec::new();
    for receipt in receipts.iter().skip(skip) {
        let step = state.steps.get(&receipt.step_id);
        let arguments = step.and_then(|step| {
            let intent = step.intent.as_ref()?;
            let material = store.load_invocation_material(&intent.effect_id).ok()??;
            let InvocationMaterialRetention::ReconstructableReference(reference) =
                material.retention()
            else {
                return None;
            };
            let catalog = catalog.as_ref()?;
            catalog
                .workspace_arguments(
                    reference,
                    &step.step_id,
                    material.material_digest(),
                    &receipt.capability.capability_id,
                )
                .or_else(|_| {
                    catalog.process_family_arguments(
                        reference,
                        &step.step_id,
                        material.material_digest(),
                        &receipt.capability,
                    )
                })
                .ok()
        });
        let output = step
            .and_then(|step| step.intent.as_ref())
            .and_then(|intent| store.load_tool_output(&intent.effect_id).ok().flatten());
        let failure = step
            .and_then(|step| step.failure_code)
            .map(|code| code.as_str().to_owned())
            .or_else(|| (receipt.status != ReceiptStatus::Succeeded).then(|| "failed".to_owned()));
        activities.push(activity_from(
            &receipt.capability.capability_id,
            arguments.as_ref(),
            output
                .as_ref()
                .map(xgen_workgraph::ToolOutputRecord::output),
            failure,
        ));
    }
    Ok(activities)
}

/// How many Receipts the Run has, so a resumed Run reports only the Steps it finishes next.
///
/// # Errors
///
/// Returns [`PublicRunError::Configuration`] when the Run does not exist and
/// [`PublicRunError::Integrity`] when its journal does not open.
pub fn inspect_receipt_count(run_id: &str) -> Result<usize, PublicRunError> {
    let root = discover_state_root().map_err(|_| PublicRunError::Configuration)?;
    let layout =
        RunLayout::existing(&root, run_id).map_err(crate::composition::map_layout_existing)?;
    SqliteRunStore::open_existing_read_only(layout.database_path())
        .and_then(|store| store.load_execution_receipts())
        .map(|receipts| receipts.len())
        .map_err(|_| PublicRunError::Integrity)
}

/// Lines removed and added by a patch's edits, as the change report counts them.
fn patch_line_counts(arguments: Option<&Value>) -> (usize, usize) {
    let edits = arguments
        .and_then(|arguments| arguments.get("edits"))
        .and_then(Value::as_array);
    edits.map_or((0, 0), |edits| {
        edits.iter().fold((0, 0), |(removed, added), edit| {
            let text = |key: &str| {
                edit.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let edit = TextEdit {
                old_text: text("oldText"),
                new_text: text("newText"),
            };
            let (old, new) = edit_lines(&edit);
            (removed + old.len(), added + new.len())
        })
    })
}

fn activity_from(
    capability_id: &str,
    arguments: Option<&Value>,
    output: Option<&Value>,
    failure: Option<String>,
) -> StepActivity {
    let text = |key: &str| {
        arguments
            .and_then(|arguments| arguments.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let path = || {
        arguments
            .and_then(display_path)
            .unwrap_or_else(|| ".".to_owned())
    };
    let output_number = |key: &str| {
        output
            .and_then(|output| output.get(key))
            .and_then(Value::as_i64)
    };
    let (verb, subject, detail) = match capability_id {
        "xgeny.fs/read-text" => ("read", path(), None),
        "xgeny.fs/read-text-lines" => (
            "read",
            path(),
            output_number("startLine")
                .zip(output_number("endLine"))
                .map(|(start, end)| format!("lines {start}-{end}")),
        ),
        "xgeny.fs/list-directory" => ("listed", path(), None),
        "xgeny.fs/stat" => ("inspected", path(), None),
        "xgeny.fs/search-text" => (
            "searched",
            format!("{:?} in {}", text("query").unwrap_or_default(), path()),
            output
                .and_then(|output| output.get("matches"))
                .and_then(Value::as_array)
                .map(|matches| format!("{} matches", matches.len())),
        ),
        APPLY_PATCH => {
            let (removed, added) = patch_line_counts(arguments);
            ("patched", path(), Some(format!("+{added} -{removed}")))
        }
        WRITE_ATOMIC => (
            "wrote",
            path(),
            text("content").map(|content| format!("{} lines", content.lines().count())),
        ),
        PROCESS_EXECUTE_CAPABILITY_ID => {
            let mut command = text("executable")
                .map(|executable| logical_executable(&executable).to_owned())
                .unwrap_or_default();
            for argument in arguments
                .and_then(|arguments| arguments.get("args"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                command.push(' ');
                command.push_str(argument);
            }
            (
                "ran",
                command,
                output_number("exitCode").map(|code| format!("exit {code}")),
            )
        }
        TERMINAL_START => (
            "started terminal",
            text("executable")
                .map(|executable| logical_executable(&executable).to_owned())
                .unwrap_or_default(),
            None,
        ),
        TERMINAL_WRITE => ("typed into terminal", String::new(), None),
        other => (
            "used",
            other.to_owned(),
            text("query").map(|query| format!("{query:?}")),
        ),
    };
    StepActivity {
        verb: verb.to_owned(),
        subject,
        detail,
        failure,
    }
}

fn change_from(
    capability_id: &str,
    arguments: &Value,
    changed: Option<bool>,
) -> Option<CommittedChange> {
    match capability_id {
        APPLY_PATCH => Some(CommittedChange::Patch {
            path: display_path(arguments)?,
            changed: changed?,
            edits: arguments
                .get("edits")?
                .as_array()?
                .iter()
                .map(|edit| {
                    Some(TextEdit {
                        old_text: edit.get("oldText")?.as_str()?.to_owned(),
                        new_text: edit.get("newText")?.as_str()?.to_owned(),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        }),
        WRITE_ATOMIC => Some(CommittedChange::Write {
            path: display_path(arguments)?,
            created: arguments.get("expectedDigest")?.is_null(),
            changed: changed?,
            content: arguments.get("content")?.as_str()?.to_owned(),
        }),
        PROCESS_EXECUTE_CAPABILITY_ID => Some(CommittedChange::Execute {
            executable: logical_executable(arguments.get("executable")?.as_str()?).to_owned(),
            args: arguments
                .get("args")?
                .as_array()?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?,
        }),
        _ => None,
    }
}

/// Approved executables are planned as `process:<id>/executables/<logical id>`; the user named
/// only the logical id.
fn logical_executable(canonical: &str) -> &str {
    canonical
        .split_once("/executables/")
        .map_or(canonical, |(_, logical)| logical)
}

/// Canonical paths are `workspace:<id>/<relative>`; the user knows only the relative part.
fn display_path(arguments: &Value) -> Option<String> {
    let canonical = arguments.get("path")?.as_str()?;
    canonical
        .split_once('/')
        .map(|(_, relative)| relative.to_owned())
}

/// Lines a patch edit removes and adds after dropping the lines both sides share at the start and
/// the end, so an edit that carries context shows only what changed.
#[must_use]
pub fn edit_lines(edit: &TextEdit) -> (Vec<&str>, Vec<&str>) {
    let old: Vec<&str> = edit.old_text.lines().collect();
    let new: Vec<&str> = edit.new_text.lines().collect();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    (
        old[prefix..old.len() - suffix].to_vec(),
        new[prefix..new.len() - suffix].to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn edit(old_text: &str, new_text: &str) -> TextEdit {
        TextEdit {
            old_text: old_text.to_owned(),
            new_text: new_text.to_owned(),
        }
    }

    #[test]
    fn activities_name_the_target_and_a_short_result() {
        let search = activity_from(
            "xgeny.fs/search-text",
            Some(&json!({"path": "workspace:primary/src", "query": "likes"})),
            Some(&json!({"matches": [{}, {}], "truncated": false})),
            None,
        );
        assert_eq!(
            (
                search.verb.as_str(),
                search.subject.as_str(),
                search.detail.as_deref()
            ),
            ("searched", "\"likes\" in src", Some("2 matches"))
        );
        let patch = activity_from(
            APPLY_PATCH,
            Some(&json!({"path": "workspace:primary/a.py", "edits": [
                {"oldText": "x\ny = 1", "newText": "x\ny = 2\nz = 3"}]})),
            None,
            None,
        );
        assert_eq!(patch.detail.as_deref(), Some("+2 -1"));
        let run = activity_from(
            PROCESS_EXECUTE_CAPABILITY_ID,
            Some(
                &json!({"executable": "process:primary/executables/bash", "args": ["-c", "pytest"]}),
            ),
            Some(&json!({"exitCode": 1})),
            None,
        );
        assert_eq!(
            (
                run.verb.as_str(),
                run.subject.as_str(),
                run.detail.as_deref()
            ),
            ("ran", "bash -c pytest", Some("exit 1"))
        );
        let missing = activity_from(
            "xgeny.fs/read-text",
            Some(&json!({"path": "workspace:primary/gone.md"})),
            None,
            Some("not_found".to_owned()),
        );
        assert_eq!(
            (missing.subject.as_str(), missing.failure.as_deref()),
            ("gone.md", Some("not_found"))
        );
    }

    #[test]
    fn edit_lines_drop_shared_context() {
        let shared = edit("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(edit_lines(&shared), (vec!["b"], vec!["B"]));
        let inserted = edit("a\nc", "a\nb\nc");
        assert_eq!(edit_lines(&inserted), (vec![], vec!["b"]));
        let one_line = edit("timeout=30", "timeout=60");
        assert_eq!(
            edit_lines(&one_line),
            (vec!["timeout=30"], vec!["timeout=60"])
        );
        let repeated = edit("x\nx", "x");
        assert_eq!(edit_lines(&repeated), (vec!["x"], vec![]));
    }

    #[test]
    fn changes_keep_the_approved_arguments_and_drop_the_workspace_prefix() {
        let patch = change_from(
            APPLY_PATCH,
            &json!({
                "path": "workspace:primary/src/lib.rs",
                "expectedDigest": format!("sha256:{}", "a".repeat(64)),
                "edits": [{"oldText": "41", "newText": "42"}]
            }),
            Some(true),
        );
        assert_eq!(
            patch,
            Some(CommittedChange::Patch {
                path: "src/lib.rs".to_owned(),
                changed: true,
                edits: vec![edit("41", "42")],
            })
        );
        let created = change_from(
            WRITE_ATOMIC,
            &json!({
                "path": "workspace:primary/notes.md",
                "content": "hello\n",
                "expectedDigest": null
            }),
            Some(true),
        );
        assert!(matches!(
            created,
            Some(CommittedChange::Write { created: true, ref path, .. }) if path == "notes.md"
        ));
        let executed = change_from(
            PROCESS_EXECUTE_CAPABILITY_ID,
            &json!({
                "executable": "process:primary/executables/cargo",
                "args": ["fmt"],
                "cwd": ".",
                "env": {}
            }),
            None,
        );
        assert_eq!(
            executed,
            Some(CommittedChange::Execute {
                executable: "cargo".to_owned(),
                args: vec!["fmt".to_owned()],
            })
        );
        assert_eq!(
            change_from(APPLY_PATCH, &json!({"path": "workspace:primary/a"}), None),
            None
        );
    }
}
