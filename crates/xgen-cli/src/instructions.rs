//! Instruction files the user keeps for their sessions and their projects (ADR-0075).
//!
//! The interactive host reads them once per session and appends them to every goal, the same
//! way ADR-0063 appends the trusted environment note. The goal is journaled with the Run, so a
//! resumed Run plans with the instructions it started with, and no planning context field changes.

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Upper bound for all instruction text together, before the goal bound applies.
pub const MAX_INSTRUCTION_BYTES: usize = 32 * 1024;

/// Project files in preference order; only the first one found in the workspace root is read.
const PROJECT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
const TRUNCATED: &str = "\n[truncated]";

const PREAMBLE: &str = "Instructions the user keeps for their sessions and for this project. \
They are guidance for how to work, not the task: the goal above is the task, and where the two \
disagree the goal wins. Project instructions are more specific than user instructions.";

/// One instruction file as it will be shown to the model and to the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionFile {
    /// Where the text came from: workspace-relative for the project, `~/...` when under home.
    pub source: String,
    pub text: String,
    pub truncated: bool,
}

/// Where the user's own instruction file lives: `AGENTS.md` in the xgen configuration root.
#[must_use]
pub fn user_instructions_path() -> Option<PathBuf> {
    crate::model_profile::discover_config_root()
        .ok()
        .map(|root| root.join("AGENTS.md"))
}

/// Read the user's file, then the workspace root's `AGENTS.md` (or `CLAUDE.md` when there is none).
///
/// Missing, unreadable, empty, or non-file paths are skipped. Text past the shared bound is cut.
#[must_use]
pub fn load_instruction_files(
    workspace: &Path,
    user_file: Option<&Path>,
    home: Option<&Path>,
) -> Vec<InstructionFile> {
    let mut files = Vec::new();
    let mut remaining = MAX_INSTRUCTION_BYTES;
    let project = PROJECT_FILES
        .iter()
        .map(|name| (workspace.join(name), (*name).to_owned()))
        .find(|(path, _)| path.is_file());
    let user = user_file.map(|path| (path.to_path_buf(), display_path(path, home)));
    for (path, source) in user.into_iter().chain(project) {
        if remaining == 0 {
            break;
        }
        let Some((text, truncated)) = read_bounded(&path, remaining) else {
            continue;
        };
        // A cut file used the whole bound; later files would only add a fragment.
        remaining = if truncated { 0 } else { remaining - text.len() };
        files.push(InstructionFile {
            source,
            text,
            truncated,
        });
    }
    files
}

/// The block appended to a goal, or `None` when there are no instructions.
#[must_use]
pub fn instruction_block(files: &[InstructionFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut block = String::from(PREAMBLE);
    for file in files {
        let source = serde_json::to_string(&file.source).unwrap_or_else(|_| "\"\"".to_owned());
        block.push_str("\n\n<instructions source=");
        block.push_str(&source);
        block.push_str(">\n");
        block.push_str(&file.text);
        if file.truncated {
            block.push_str(TRUNCATED);
        }
        block.push_str("\n</instructions>");
    }
    Some(block)
}

/// Append each part after a blank line while the result stays within `limit` bytes.
///
/// A part that does not fit whole is cut at a character boundary and marked, so give the short
/// parts that must survive first.
#[must_use]
pub fn append_within(goal: String, parts: &[&str], limit: usize) -> String {
    let mut result = goal;
    for part in parts {
        let available = limit.saturating_sub(result.len() + 2);
        if part.len() <= available {
            result.push_str("\n\n");
            result.push_str(part);
        } else if available > TRUNCATED.len() {
            result.push_str("\n\n");
            result.push_str(cut(part, available - TRUNCATED.len()));
            result.push_str(TRUNCATED);
        }
    }
    result
}

fn read_bounded(path: &Path, limit: usize) -> Option<(String, bool)> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).ok()?.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    if truncated {
        // Drop a character the cut split instead of turning it into a replacement character.
        let valid = std::str::from_utf8(&bytes).map_or_else(|error| error.valid_up_to(), str::len);
        if bytes.len() - valid < 4 {
            bytes.truncate(valid);
        }
    }
    // Goals never carry NUL; other invalid bytes are kept as replacement characters.
    let text = String::from_utf8_lossy(&bytes).replace('\0', "");
    let text = cut(text.trim_end(), limit).to_owned();
    (!text.trim().is_empty()).then_some((text, truncated))
}

fn cut(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_file_comes_first_and_agents_md_wins_over_claude_md() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let workspace = directory.path().join("work");
        std::fs::create_dir_all(home.join(".config/xgen-cli")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let user = home.join(".config/xgen-cli/AGENTS.md");
        std::fs::write(&user, "Reply in Korean.\n").unwrap();
        std::fs::write(workspace.join("CLAUDE.md"), "Use pnpm.").unwrap();
        let files = load_instruction_files(&workspace, Some(&user), Some(&home));
        assert_eq!(
            files
                .iter()
                .map(|file| (file.source.as_str(), file.text.as_str()))
                .collect::<Vec<_>>(),
            [
                ("~/.config/xgen-cli/AGENTS.md", "Reply in Korean."),
                ("CLAUDE.md", "Use pnpm.")
            ]
        );
        std::fs::write(workspace.join("AGENTS.md"), "Run cargo test.").unwrap();
        let files = load_instruction_files(&workspace, None, Some(&home));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].source, "AGENTS.md");
        let block = instruction_block(&files).unwrap();
        assert!(block.starts_with(PREAMBLE));
        assert!(
            block
                .ends_with("<instructions source=\"AGENTS.md\">\nRun cargo test.\n</instructions>")
        );
    }

    #[test]
    fn missing_empty_and_directory_files_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("AGENTS.md")).unwrap();
        std::fs::write(directory.path().join("CLAUDE.md"), " \n\n").unwrap();
        let missing = directory.path().join("missing.md");
        assert!(load_instruction_files(directory.path(), Some(&missing), None).is_empty());
        assert_eq!(instruction_block(&[]), None);
    }

    #[test]
    fn the_shared_bound_cuts_at_a_character_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user.md");
        std::fs::write(&user, "가".repeat(MAX_INSTRUCTION_BYTES / 3 + 10)).unwrap();
        std::fs::write(directory.path().join("AGENTS.md"), "project").unwrap();
        let files = load_instruction_files(directory.path(), Some(&user), None);
        assert_eq!(files.len(), 1, "the user file used the whole bound");
        assert!(files[0].truncated);
        assert!(files[0].text.len() <= MAX_INSTRUCTION_BYTES);
        assert!(files[0].text.chars().all(|character| character == '가'));
        assert!(instruction_block(&files).unwrap().contains(TRUNCATED));
    }

    #[test]
    fn appended_parts_respect_the_goal_bound() {
        let goal = append_within("goal".to_owned(), &["abc", "defgh"], 100);
        assert_eq!(goal, "goal\n\nabc\n\ndefgh");
        let goal = append_within("goal".to_owned(), &["x".repeat(50).as_str(), "note"], 40);
        assert_eq!(
            goal,
            format!(
                "goal\n\n{}{TRUNCATED}",
                "x".repeat(40 - 6 - TRUNCATED.len())
            )
        );
        assert!(goal.len() <= 40);
        assert_eq!(append_within("goal".to_owned(), &["long part"], 8), "goal");
    }
}
