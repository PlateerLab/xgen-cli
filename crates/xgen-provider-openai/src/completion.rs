//! Host-owned completion requirements, independent of application semantics.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::OpenAiPlannerConfigError;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionCheck {
    pub id: String,
    pub argv: Vec<String>,
}

impl std::fmt::Debug for CompletionCheck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted completion check>")
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionContract {
    pub response_schema: serde_json::Value,
    pub checks: Vec<CompletionCheck>,
}

/// Parse one bounded, duplicate-key-free local completion contract.
/// # Errors
/// Rejects unsupported schemas and empty, duplicate or oversized checks.
pub fn parse_completion_contract(
    encoded: &str,
) -> Result<CompletionContract, OpenAiPlannerConfigError> {
    let invalid = || OpenAiPlannerConfigError::InvalidProfileField("completion_contract");
    if encoded.len() > 32_768 {
        return Err(invalid());
    }
    let value = super::parse_unique_json(encoded.as_bytes(), 32).map_err(|_| invalid())?;
    let contract: CompletionContract = serde_json::from_value(value).map_err(|_| invalid())?;
    super::parse_completion_schema(&contract.response_schema.to_string())?;
    validate_completion_checks(&contract.checks)?;
    Ok(contract)
}

pub(crate) fn validate_completion_checks(
    checks: &[CompletionCheck],
) -> Result<(), OpenAiPlannerConfigError> {
    let invalid = || OpenAiPlannerConfigError::InvalidProfileField("completion_checks");
    if checks.is_empty() || checks.len() > 8 {
        return Err(invalid());
    }
    let mut ids = BTreeSet::new();
    let mut commands = BTreeSet::new();
    for check in checks {
        if check.id.is_empty()
            || check.id.len() > 64
            || !check
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            || !ids.insert(&check.id)
            || !commands.insert(&check.argv)
            || check.argv.is_empty()
            || check.argv.len() > 64
            || check
                .argv
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(invalid());
        }
        let executable = &check.argv[0];
        if executable.is_empty()
            || executable.len() > 128
            || !executable.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || (i > 0 && matches!(b, b'.' | b'_' | b'+' | b'-'))
            })
        {
            return Err(invalid());
        }
    }
    if serde_json::to_vec(checks).map_err(|_| invalid())?.len() > 32_768 {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contracts_are_bounded_unique_and_require_concrete_argv() {
        for argv in [
            json!(["verify", "--strict"]),
            json!(["python3", "-B", "check.py"]),
        ] {
            let valid =
                json!({"response_schema":{"type":"object"},"checks":[{"id":"result","argv":argv}]});
            assert!(parse_completion_contract(&valid.to_string()).is_ok());
            for broken in [
                json!({"response_schema":{"type":"object"},"checks":[]}),
                json!({"response_schema":{"type":"array"},"checks":valid["checks"]}),
                json!({"response_schema":{"type":"object"},"checks":[valid["checks"][0],valid["checks"][0]]}),
                json!({"response_schema":{"type":"object"},"checks":[{"id":"result","argv":[]}]}),
                json!({"response_schema":{"type":"object"},"checks":[{"id":"result","argv":["/bin/true"]}]}),
                json!({"response_schema":{"type":"object"},"checks":[{"id":"result","argv":["verify"],"expected_exit":1}]}),
            ] {
                assert!(parse_completion_contract(&broken.to_string()).is_err());
            }
        }
        assert!(
            parse_completion_contract(
                r#"{"response_schema":{"type":"object"},"checks":[],"checks":[]}"#
            )
            .is_err()
        );
        assert!(parse_completion_contract(&" ".repeat(32_769)).is_err());
    }

    #[test]
    fn required_checks_change_request_profile_and_cannot_replace_schema() {
        let base = || {
            crate::OpenAiPlannerConfig::new(
                "http://127.0.0.1:1/v1",
                "planner",
                "model",
                "tokenizer",
            )
            .unwrap()
        };
        let checks = vec![CompletionCheck {
            id: "result".into(),
            argv: vec!["verify".into()],
        }];
        assert!(base().with_completion_checks(&checks).is_err());
        let schema = base()
            .with_completion_schema(r#"{"type":"object"}"#)
            .unwrap();
        let original_digest = schema.request_profile_digest().to_owned();
        let bound = schema.with_completion_checks(&checks).unwrap();
        assert_ne!(bound.request_profile_digest(), original_digest);
        assert!(bound.system_prompt().contains("COMPLETION_CHECKS_V1"));
        assert!(!format!("{bound:?}").contains("result"));
    }
}
