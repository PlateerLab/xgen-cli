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
    let layout = RunLayout::existing(&root, run_id).map_err(|_| PublicRunError::Configuration)?;
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
