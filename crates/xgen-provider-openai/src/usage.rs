use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Provider-reported counts, never inferred from text length or missing fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

/// Adapter classification, independent of subsequent Core proposal admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallOutcome {
    ProposalDecoded,
    ResponseRejected,
    TransportFailed,
}

/// Content-free observation of one completed HTTP attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCallObservation {
    pub call_id: String,
    pub request_digest: String,
    pub elapsed_millis: u64,
    pub outcome: ModelCallOutcome,
    pub usage: Option<TokenUsage>,
}

pub(super) fn decode_usage(body: &[u8], max_depth: usize, model: &str) -> Option<TokenUsage> {
    let envelope = super::parse_unique_json(body, max_depth).ok()?;
    if envelope.get("model")?.as_str()? != model {
        return None;
    }
    let usage = envelope.get("usage")?.as_object()?;
    let input_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let output_tokens = usage.get("completion_tokens")?.as_u64()?;
    let total_tokens = usage.get("total_tokens")?.as_u64()?;
    if input_tokens.checked_add(output_tokens)? != total_tokens {
        return None;
    }
    let standard_cache =
        optional_count(usage.get("prompt_tokens_details"), "cached_tokens").ok()?;
    let native_cache = optional_scalar(usage.get("prompt_cache_hit_tokens")).ok()?;
    if standard_cache
        .zip(native_cache)
        .is_some_and(|(a, b)| a != b)
    {
        return None;
    }
    let cached_input_tokens = standard_cache.or(native_cache);
    if cached_input_tokens.is_some_and(|count| count > input_tokens) {
        return None;
    }
    if let Some(miss) = optional_scalar(usage.get("prompt_cache_miss_tokens")).ok()?
        && (miss > input_tokens
            || cached_input_tokens.is_some_and(|hit| hit.checked_add(miss) != Some(input_tokens)))
    {
        return None;
    }
    let reasoning_tokens =
        optional_count(usage.get("completion_tokens_details"), "reasoning_tokens").ok()?;
    if reasoning_tokens.is_some_and(|count| count > output_tokens) {
        return None;
    }
    Some(TokenUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        reasoning_tokens,
    })
}

fn optional_scalar(value: Option<&Value>) -> Result<Option<u64>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(()),
    }
}

fn optional_count(value: Option<&Value>, key: &str) -> Result<Option<u64>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => optional_scalar(value.as_object().ok_or(())?.get(key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(usage: &Value) -> Option<TokenUsage> {
        decode_usage(
            &serde_json::to_vec(&json!({"model":"test", "usage":usage})).unwrap(),
            64,
            "test",
        )
    }

    #[test]
    fn standard_and_native_cache_counts_agree_without_double_counting() {
        let standard = decode(&json!({"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,
            "prompt_tokens_details":{"cached_tokens":60},"completion_tokens_details":{"reasoning_tokens":10}})).unwrap();
        let native = decode(&json!({"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,
            "prompt_cache_hit_tokens":60,"prompt_cache_miss_tokens":40,
            "prompt_tokens_details":{"cached_tokens":60},"completion_tokens_details":{"reasoning_tokens":10}})).unwrap();
        assert_eq!(standard, native);
        assert_eq!(native.cached_input_tokens, Some(60));
        assert_eq!(native.total_tokens, 120);
    }

    #[test]
    fn unknown_cache_and_explicit_zero_are_distinct() {
        let unknown =
            decode(&json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12})).unwrap();
        assert_eq!(unknown.cached_input_tokens, None);
        let zero = decode(&json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_cache_hit_tokens":0})).unwrap();
        assert_eq!(zero.cached_input_tokens, Some(0));
        let miss_only = decode(&json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_cache_miss_tokens":10})).unwrap();
        assert_eq!(miss_only.input_tokens, 10);
        assert_eq!(miss_only.cached_input_tokens, None);
    }

    #[test]
    fn malformed_and_inconsistent_counts_are_unknown() {
        for usage in [
            json!({"prompt_tokens":-1,"completion_tokens":2,"total_tokens":1}),
            json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":99}),
            json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_cache_hit_tokens":11}),
            json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_cache_hit_tokens":5,"prompt_tokens_details":{"cached_tokens":6}}),
            json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"completion_tokens_details":{"reasoning_tokens":3}}),
            json!({"prompt_tokens":u64::MAX,"completion_tokens":2,"total_tokens":1}),
        ] {
            assert_eq!(decode(&usage), None);
        }
    }
}
