//! Bounded, sanitized summary of a provider's 4xx answer, for human display only.
//!
//! The text is never written to the Run journal and never sent back to a model. It exists so a
//! person can see why the provider refused a request (wrong key, unknown model, unsupported
//! parameter, quota) instead of a bare failure class.

use std::fmt;

use serde_json::Value;

/// Bytes of a non-success body read for a rejection summary.
pub(crate) const MAX_REJECTION_BODY_BYTES: u64 = 16 * 1024;
const MAX_MESSAGE_CHARS: usize = 300;
const MAX_LABEL_CHARS: usize = 64;
const REDACTED: &str = "[redacted]";

/// What a provider said when it refused a request, after redaction and truncation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRejection {
    status: u16,
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

impl ProviderRejection {
    /// Summarize one non-success response. `secret` is removed verbatim before any other rule.
    #[must_use]
    pub fn from_response(status: u16, body: &[u8], secret: Option<&str>) -> Self {
        let parsed = serde_json::from_slice::<Value>(body).ok();
        let (kind, code, message) = match parsed.as_ref().and_then(error_object) {
            Some(Fields {
                kind,
                code,
                message,
            }) => (kind, code, message),
            None if parsed.is_none() => (None, None, plain_text(body)),
            None => (None, None, None),
        };
        Self {
            status,
            kind: kind.and_then(|text| sanitize(&text, MAX_LABEL_CHARS, secret)),
            code: code.and_then(|text| sanitize(&text, MAX_LABEL_CHARS, secret)),
            message: message.and_then(|text| sanitize(&text, MAX_MESSAGE_CHARS, secret)),
        }
    }

    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Provider error type, such as `invalid_request_error` or `INVALID_ARGUMENT`.
    #[must_use]
    pub fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    /// Provider error code, such as `model_not_found` or `invalid_api_key`.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The provider refused the credential rather than the request content.
    #[must_use]
    pub const fn is_authentication(&self) -> bool {
        matches!(self.status, 401 | 403)
    }
}

impl fmt::Display for ProviderRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "HTTP {}", self.status)?;
        let labels = [self.kind.as_deref(), self.code.as_deref()];
        let mut labels = labels.iter().flatten().peekable();
        if labels.peek().is_some() {
            formatter.write_str(" ")?;
            for (index, label) in labels.enumerate() {
                if index > 0 {
                    formatter.write_str("/")?;
                }
                formatter.write_str(label)?;
            }
        }
        if let Some(message) = &self.message {
            write!(formatter, ": {message}")?;
        }
        Ok(())
    }
}

struct Fields {
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

/// Find the error object across the common shapes: an `error` object (OpenAI-compatible APIs and
/// Anthropic), a one-element list wrapping it (Google), a plain `error` string (Ollama) and a
/// top-level `message` (vLLM), plus a `FastAPI` `detail` (Mistral and many self-hosted servers).
fn error_object(value: &Value) -> Option<Fields> {
    let value = match value {
        Value::Array(items) => items.first()?,
        other => other,
    };
    let object = match value.get("error") {
        Some(Value::String(text)) => {
            return Some(Fields {
                kind: None,
                code: None,
                message: Some(text.clone()),
            });
        }
        Some(error @ Value::Object(_)) => error,
        _ if value.get("message").is_some() => value,
        _ => return detail(value.get("detail")?),
    };
    Some(Fields {
        kind: label(object.get("type")).or_else(|| label(object.get("status"))),
        code: label(object.get("code")),
        message: object
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// FastAPI-style `detail`: a sentence, or a validation list whose first entry has `msg`.
fn detail(value: &Value) -> Option<Fields> {
    let message = match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.first()?.get("msg")?.as_str()?.to_owned(),
        _ => return None,
    };
    Some(Fields {
        kind: None,
        code: None,
        message: Some(message),
    })
}

fn label(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A short non-JSON answer (a proxy or gateway sentence). Markup pages are not summarized.
fn plain_text(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_owned())
}

/// Remove control characters, collapse whitespace, redact secret-shaped tokens and truncate.
fn sanitize(text: &str, max_chars: usize, secret: Option<&str>) -> Option<String> {
    let text = match secret {
        Some(secret) if secret.len() >= 8 => text.replace(secret, REDACTED),
        _ => text.to_owned(),
    };
    // Invisible and direction-changing characters could disguise the text on a terminal.
    let text: String = text
        .chars()
        .filter(|character| {
            !matches!(
                character,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
            )
        })
        .collect();
    let mut words = Vec::new();
    let mut redact_next = false;
    for word in text
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
    {
        if redact_next {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        redact_next = word.eq_ignore_ascii_case("bearer");
        words.push(redact_word(word));
    }
    let joined = words.join(" ");
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() <= max_chars {
        return Some(joined);
    }
    let mut truncated: String = joined.chars().take(max_chars).collect();
    truncated.push_str("...");
    Some(truncated)
}

fn redact_word(word: &str) -> String {
    let mut output = String::with_capacity(word.len());
    let mut run = String::new();
    for character in word.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            run.push(character);
        } else {
            flush_run(&mut output, &mut run);
            output.push(character);
        }
    }
    flush_run(&mut output, &mut run);
    output
}

fn flush_run(output: &mut String, run: &mut String) {
    if secret_like(run) {
        output.push_str(REDACTED);
    } else {
        output.push_str(run);
    }
    run.clear();
}

/// Credential-shaped runs: well-known key prefixes, long mixed-case alphanumerics, long hex.
/// Model identifiers are lowercase with separators and stay readable.
fn secret_like(run: &str) -> bool {
    const PREFIXES: [&str; 8] = ["sk-", "sk_", "pk-", "AIza", "ghp_", "gho_", "hf_", "xox"];
    // Providers echo a masked key (`sk-abcd****wxyz`); its visible prefix is still key material.
    if run.len() >= 6 && PREFIXES.iter().any(|prefix| run.starts_with(prefix)) {
        return true;
    }
    let has_upper = run.chars().any(|character| character.is_ascii_uppercase());
    let has_lower = run.chars().any(|character| character.is_ascii_lowercase());
    let has_digit = run.chars().any(|character| character.is_ascii_digit());
    if run.len() >= 24 && has_upper && has_lower && has_digit {
        return true;
    }
    run.len() >= 32 && run.chars().all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::ProviderRejection;

    fn summary(status: u16, body: &str) -> String {
        ProviderRejection::from_response(status, body.as_bytes(), None).to_string()
    }

    #[test]
    fn openai_and_deepseek_error_objects_keep_type_code_and_message() {
        assert_eq!(
            summary(
                404,
                r#"{"error":{"message":"The model `gpt-9` does not exist or you do not have access to it.","type":"invalid_request_error","param":null,"code":"model_not_found"}}"#
            ),
            "HTTP 404 invalid_request_error/model_not_found: The model `gpt-9` does not exist or you do not have access to it."
        );
        assert_eq!(
            summary(
                401,
                r#"{"error":{"message":"Authentication Fails, Your api key: ****abcd is invalid","type":"authentication_error","param":null,"code":"invalid_request_error"}}"#
            ),
            "HTTP 401 authentication_error/invalid_request_error: Authentication Fails, Your api key: ****abcd is invalid"
        );
    }

    #[test]
    fn openrouter_numeric_codes_and_anthropic_types_are_labels() {
        assert_eq!(
            summary(
                400,
                r#"{"error":{"code":400,"message":"deepseek/deepseek-chat-v3 is not a valid model ID","metadata":{"provider_name":null}}}"#
            ),
            "HTTP 400 400: deepseek/deepseek-chat-v3 is not a valid model ID"
        );
        assert_eq!(
            summary(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: Field required"}}"#
            ),
            "HTTP 400 invalid_request_error: max_tokens: Field required"
        );
    }

    #[test]
    fn google_list_ollama_string_and_vllm_top_level_shapes() {
        assert_eq!(
            summary(
                400,
                r#"[{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}]"#
            ),
            "HTTP 400 INVALID_ARGUMENT/400: API key not valid. Please pass a valid API key."
        );
        assert_eq!(
            summary(
                404,
                r#"{"error":"model \"llama9\" not found, try pulling it first"}"#
            ),
            "HTTP 404: model \"llama9\" not found, try pulling it first"
        );
        assert_eq!(
            summary(
                400,
                r#"{"object":"error","message":"This model's maximum context length is 8192 tokens.","type":"BadRequestError","param":null,"code":400}"#
            ),
            "HTTP 400 BadRequestError/400: This model's maximum context length is 8192 tokens."
        );
    }

    #[test]
    fn fastapi_detail_sentences_and_validation_lists() {
        assert_eq!(
            summary(401, r#"{"detail":"Invalid API Key"}"#),
            "HTTP 401: Invalid API Key"
        );
        assert_eq!(
            summary(
                422,
                r#"{"detail":[{"loc":["body","model"],"msg":"field required","type":"value_error.missing"}]}"#
            ),
            "HTTP 422: field required"
        );
    }

    #[test]
    fn masked_key_echoes_lose_their_visible_prefix() {
        let text = summary(
            401,
            r#"{"error":{"message":"Incorrect API key provided: sk-thisi**********AbC1.","code":"invalid_api_key"}}"#,
        );
        assert!(!text.contains("thisi"), "{text}");
        assert!(text.contains("[redacted]**********AbC1"), "{text}");
    }

    #[test]
    fn unknown_markup_and_empty_bodies_keep_only_the_status() {
        assert_eq!(
            summary(403, "<html><body>Forbidden</body></html>"),
            "HTTP 403"
        );
        assert_eq!(summary(400, ""), "HTTP 400");
        assert_eq!(summary(400, r#"{"detail":{"x":1}}"#), "HTTP 400");
        assert_eq!(
            summary(400, "Bad Request: unknown field"),
            "HTTP 400: Bad Request: unknown field"
        );
    }

    #[test]
    fn keys_tokens_and_control_characters_are_removed() {
        let key = "sk-proj-Ab3dEf6hIj9kLm2nOp5qRs8t";
        let body = format!(
            r#"{{"error":{{"message":"Incorrect API key provided: {key}.\u001b[31m Bearer abc.def.ghi\nhex 0123456789abcdef0123456789abcdef AIzaSyA1b2C3d4E5","type":"invalid_request_error"}}}}"#
        );
        let text = ProviderRejection::from_response(401, body.as_bytes(), None).to_string();
        assert!(!text.contains("Ab3dEf6"), "{text}");
        assert!(!text.contains("abc.def"), "{text}");
        assert!(!text.contains("0123456789abcdef"), "{text}");
        assert!(!text.contains("AIzaSy"), "{text}");
        assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text}");

        let hidden = ProviderRejection::from_response(
            400,
            "{\"error\":{\"message\":\"safe\u{202e}txt.exe\u{200b} name\"}}".as_bytes(),
            None,
        );
        assert_eq!(hidden.message(), Some("safetxt.exe name"));
        assert!(
            text.contains("Incorrect API key provided: [redacted]."),
            "{text}"
        );

        let exact = ProviderRejection::from_response(
            400,
            br#"{"error":{"message":"token plainsecretvalue rejected"}}"#,
            Some("plainsecretvalue"),
        );
        assert_eq!(exact.message(), Some("token [redacted] rejected"));
    }

    #[test]
    fn model_identifiers_stay_readable_and_long_messages_are_truncated() {
        let text = summary(
            400,
            r#"{"error":{"message":"deepseek-reasoner-2025-01-20 and gemini-3.8-flash-low are unsupported"}}"#,
        );
        assert!(text.contains("deepseek-reasoner-2025-01-20"), "{text}");
        assert!(text.contains("gemini-3.8-flash-low"), "{text}");

        let long = format!(r#"{{"error":{{"message":"{}"}}}}"#, "word ".repeat(200));
        let rejection = ProviderRejection::from_response(400, long.as_bytes(), None);
        let message = rejection.message().unwrap();
        assert!(message.ends_with("..."));
        assert_eq!(message.chars().count(), 303);
    }
}
