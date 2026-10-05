//! Host-approved HTTP GET connections. Credentials never enter graph or model material.
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::time::Duration;

use crate::graph_discovery::{self, DiscoveryCatalog};
use crate::model_profile::{ModelCredentialStore, OsModelCredentialStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;
use xgen_runtime::{
    AdapterEvidenceDigest, AdapterExecutionObservation, AdapterPrepareFailure, AdapterToolOutput,
    PreparedAdapterInvocation,
};
use zeroize::Zeroizing;

pub(crate) const READ: &str = "xgen.http/read";
pub(crate) const SCOPE: &str = "http.read";
pub(crate) type Connections = BTreeMap<String, Connection>;
const MAX_BODY: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    format_version: u32,
    base_url: String,
    allow_get: bool,
    bearer_env: Option<String>,
    credential_ref: Option<String>,
}
impl Connection {
    fn validate(&self) -> Result<(), &'static str> {
        let url = Url::parse(&self.base_url).map_err(|_| "invalid_http_base_url")?;
        if self.format_version != 1
            || !self.allow_get
            || self.base_url.len() > 1024
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.host_str().is_none()
            || !(url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))))
            || unsafe_path(&self.base_url)
            || unsafe_path(url.path())
        {
            return Err("invalid_http_base_url");
        }
        if self.bearer_env.as_ref().is_some_and(|name| {
            name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        }) || self.credential_ref.as_ref().is_some_and(|reference| {
            !reference.starts_with("cred-")
                || reference.len() != 37
                || !reference[5..].bytes().all(|b| b.is_ascii_hexdigit())
        }) || (self.bearer_env.is_some() && self.credential_ref.is_some())
        {
            return Err("invalid_http_auth_reference");
        }
        Ok(())
    }
    fn credential(&self) -> Result<Option<Zeroizing<String>>, &'static str> {
        let secret = if let Some(name) = &self.bearer_env {
            Some(Zeroizing::new(
                std::env::var(name).map_err(|_| "http_credential_unavailable")?,
            ))
        } else if let Some(reference) = &self.credential_ref {
            Some(
                OsModelCredentialStore
                    .get(reference)
                    .map_err(|_| "http_credential_unavailable")?,
            )
        } else {
            None
        };
        if secret.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 4096 || !s.bytes().all(|b| b.is_ascii_graphic())
        }) {
            return Err("http_credential_invalid");
        }
        Ok(secret)
    }
}

pub(crate) fn valid_connections(connections: &Connections) -> bool {
    connections.len() <= 16
        && connections.iter().all(|(name, connection)| {
            graph_discovery::valid_name(name) && connection.validate().is_ok()
        })
}

fn connection_root() -> Result<std::path::PathBuf, &'static str> {
    let state = crate::run_layout::discover_state_root().map_err(|_| "invalid_state_home")?;
    Ok(state.join("http-connections"))
}

pub(crate) fn connect(
    name: &str,
    base_url: String,
    bearer_env: Option<String>,
    token_stdin: bool,
    bearer_prompt: bool,
) -> Result<Value, &'static str> {
    if !graph_discovery::valid_name(name)
        || (u8::from(bearer_env.is_some()) + u8::from(token_stdin) + u8::from(bearer_prompt)) > 1
    {
        return Err("invalid_http_connection");
    }
    let mut connection = Connection {
        format_version: 1,
        base_url,
        allow_get: true,
        bearer_env,
        credential_ref: None,
    };
    connection.validate()?;
    let snapshots = DiscoveryCatalog::discover().map_err(|()| "http_catalog_unavailable")?;
    if !snapshots.contains_key(name) {
        return Err("collection_not_found");
    }
    let root = connection_root()?;
    crate::run_layout::ensure_private_state_root(root.parent().ok_or("invalid_state_home")?)
        .map_err(|_| "invalid_state_home")?;
    if !root.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&root)
            .map_err(|_| "http_connection_store_unavailable")?;
    }
    private_file_or_directory(&root, true)?;
    let path = root.join(format!("{name}.json"));
    if path.exists() {
        return Err("http_connection_already_exists");
    }
    let secret = if token_stdin {
        let mut bytes = Zeroizing::new(Vec::new());
        std::io::stdin()
            .take(4098)
            .read_to_end(&mut bytes)
            .map_err(|_| "http_credential_input_failed")?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| "http_credential_invalid")?
            .trim_end_matches(['\r', '\n']);
        Some(Zeroizing::new(text.to_owned()))
    } else if bearer_prompt {
        Some(Zeroizing::new(
            rpassword::prompt_password("API bearer token: ")
                .map_err(|_| "http_credential_input_failed")?,
        ))
    } else {
        None
    };
    if let Some(secret) = &secret {
        if secret.is_empty() || secret.len() > 4096 || !secret.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err("http_credential_invalid");
        }
        let reference = crate::model_profile::new_credential_reference()
            .map_err(|_| "http_credential_store_unavailable")?;
        OsModelCredentialStore
            .put(&reference, secret)
            .map_err(|_| "http_credential_store_unavailable")?;
        connection.credential_ref = Some(reference);
    }
    let saved: Result<(), &'static str> = (|| {
        let mut file = tempfile::NamedTempFile::new_in(&root)
            .map_err(|_| "http_connection_store_unavailable")?;
        file.write_all(
            &serde_jcs::to_vec(&connection).map_err(|_| "http_connection_store_unavailable")?,
        )
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "http_connection_store_unavailable")?;
        file.persist_noclobber(&path)
            .map_err(|_| "http_connection_store_unavailable")?;
        Ok(())
    })();
    if saved.is_err()
        && let Some(reference) = &connection.credential_ref
    {
        let _ = OsModelCredentialStore.delete(reference);
    }
    saved?;
    Ok(
        json!({"ok":true,"collection":name,"connected":true,"methods":["GET"],"authentication":if secret.is_some() {"os_secret_store"} else if connection.bearer_env.is_some() {"local_environment"} else {"none"}}),
    )
}

fn private_file_or_directory(path: &std::path::Path, directory: bool) -> Result<(), &'static str> {
    let meta = fs::symlink_metadata(path).map_err(|_| "http_connection_store_unavailable")?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err("http_connection_store_invalid");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.mode() & 0o077 != 0 {
            return Err("http_connection_store_not_private");
        }
    }
    Ok(())
}

pub(crate) fn discover(snapshots: &graph_discovery::Snapshots) -> Result<Connections, ()> {
    let root = connection_root().map_err(|_| ())?;
    if !root.exists() {
        return Ok(Connections::new());
    }
    private_file_or_directory(root.parent().ok_or(())?, true).map_err(|_| ())?;
    private_file_or_directory(&root, true).map_err(|_| ())?;
    let mut connections = Connections::new();
    for name in snapshots.keys() {
        let path = root.join(format!("{name}.json"));
        if !path.exists() {
            continue;
        }
        private_file_or_directory(&path, false).map_err(|_| ())?;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| ())?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes.len() > 4096 {
            return Err(());
        }
        let connection: Connection = serde_json::from_slice(&bytes).map_err(|_| ())?;
        connection.validate().map_err(|_| ())?;
        connections.insert(name.clone(), connection);
    }
    Ok(connections)
}

pub(crate) fn accepts(catalog: &DiscoveryCatalog, input: &Value) -> bool {
    input.as_object().is_some_and(|o| o.len() == 4)
        && input["collection"]
            .as_str()
            .is_some_and(|name| catalog.connection(name).is_some())
        && input["tool"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
        && input["contractDigest"].as_str().is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
        && input["parameters"]
            .as_object()
            .is_some_and(|o| o.len() == 2 && o["path"].is_object() && o["query"].is_object())
        && serde_jcs::to_vec(input).is_ok_and(|b| b.len() <= 8192)
}

fn descriptor(catalog: &DiscoveryCatalog, input: &Value) -> Result<Value, &'static str> {
    let output = catalog.execution_contract(
        input["collection"].as_str().ok_or("http_input_invalid")?,
        input["tool"].as_str().ok_or("http_input_invalid")?,
    )?;
    let descriptor = &output["http_read"];
    if descriptor["supported"] != true {
        return Err("http_contract_unsupported");
    }
    if descriptor["contract_digest"] != input["contractDigest"] {
        return Err("http_contract_changed");
    }
    Ok(descriptor["contract"].clone())
}

fn unsafe_path(path: &str) -> bool {
    path.contains('\\')
        || path.contains('%')
        || path.contains('?')
        || path.contains('#')
        || path.split('/').any(|s| matches!(s, "." | ".."))
}
fn scalar(value: &Value) -> Result<String, &'static str> {
    match value {
        Value::String(v) => Ok(v.clone()),
        Value::Number(_) | Value::Bool(_) => Ok(value.to_string()),
        _ => Err("http_parameter_type_invalid"),
    }
}

fn request_url(
    connection: &Connection,
    contract: &Value,
    input: &Value,
) -> Result<Url, &'static str> {
    let path = contract["path"].as_str().ok_or("http_path_invalid")?;
    if !path.starts_with('/') || path.starts_with("//") || unsafe_path(path) {
        return Err("http_path_invalid");
    }
    let definitions = contract["parameters"]
        .as_array()
        .ok_or("http_contract_invalid")?;
    if definitions.len() > 64 {
        return Err("http_parameters_limit");
    }
    let arguments = &input["parameters"];
    for location in ["path", "query"] {
        for name in arguments[location]
            .as_object()
            .ok_or("http_input_invalid")?
            .keys()
        {
            if !definitions
                .iter()
                .any(|p| p["name"] == name.as_str() && p["location"] == location)
            {
                return Err("http_parameter_unknown");
            }
        }
    }
    for definition in definitions {
        let location = definition["location"]
            .as_str()
            .ok_or("http_contract_invalid")?;
        let name = definition["name"].as_str().ok_or("http_contract_invalid")?;
        let value = arguments.get(location).and_then(|object| object.get(name));
        jsonschema::meta::options()
            .validate(&definition["schema"])
            .map_err(|_| "http_schema_unsupported")?;
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .offline()
            .build(&definition["schema"])
            .map_err(|_| "http_schema_unsupported")?;
        if let Some(value) = value {
            if !validator.is_valid(value) {
                return Err("http_parameter_schema_invalid");
            }
            let text = scalar(value)?;
            if text.len() > 2048 || text.chars().any(char::is_control) {
                return Err("http_parameter_invalid");
            }
        } else if definition["required"] == true {
            return Err("http_parameter_required");
        }
    }
    let mut url = Url::parse(&connection.base_url).map_err(|_| "http_target_invalid")?;
    let base = url.path().trim_end_matches('/').to_owned();
    url.set_path(&base);
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| "http_target_invalid")?;
        segments.pop_if_empty();
        let base_path = contract["base_path"].as_str().unwrap_or("");
        if unsafe_path(base_path) {
            return Err("http_base_path_invalid");
        }
        for segment in base_path.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
        for segment in path.trim_start_matches('/').split('/') {
            let text = if segment.starts_with('{') && segment.ends_with('}') {
                scalar(&arguments["path"][&segment[1..segment.len() - 1]])?
            } else if segment.contains(['{', '}']) {
                return Err("http_path_template_unsupported");
            } else {
                segment.to_owned()
            };
            if text.is_empty()
                || matches!(text.as_str(), "." | "..")
                || text.contains(['/', '\\', '%'])
            {
                return Err("http_path_parameter_invalid");
            }
            segments.push(&text);
        }
    }
    for (name, value) in arguments["query"].as_object().ok_or("http_input_invalid")? {
        url.query_pairs_mut().append_pair(name, &scalar(value)?);
    }
    if url.as_str().len() > 8192 {
        return Err("http_target_size_limit");
    }
    Ok(url)
}

pub(crate) fn prepare(
    catalog: &DiscoveryCatalog,
    input: &Value,
) -> Result<Box<dyn PreparedAdapterInvocation>, AdapterPrepareFailure> {
    if !accepts(catalog, input) {
        return Err(AdapterPrepareFailure::InvalidMaterial);
    }
    let contract =
        descriptor(catalog, input).map_err(|_| AdapterPrepareFailure::InvalidMaterial)?;
    let connection = catalog
        .connection(
            input["collection"]
                .as_str()
                .ok_or(AdapterPrepareFailure::InvalidMaterial)?,
        )
        .ok_or(AdapterPrepareFailure::ResourceUnavailable)?
        .clone();
    let url = request_url(&connection, &contract, input)
        .map_err(|_| AdapterPrepareFailure::InvalidMaterial)?;
    for schema in contract["responses"]
        .as_object()
        .ok_or(AdapterPrepareFailure::InvalidMaterial)?
        .values()
    {
        jsonschema::meta::options()
            .validate(schema)
            .map_err(|_| AdapterPrepareFailure::InvalidMaterial)?;
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .offline()
            .build(schema)
            .map_err(|_| AdapterPrepareFailure::InvalidMaterial)?;
    }
    if contract["method"] != "GET" {
        return Err(AdapterPrepareFailure::UnsupportedProtocol);
    }
    let credential = connection
        .credential()
        .map_err(|_| AdapterPrepareFailure::ResourceUnavailable)?;
    if contract["bearer_required"] == true && credential.is_none() {
        return Err(AdapterPrepareFailure::ResourceUnavailable);
    }
    Ok(Box::new(PreparedRead {
        catalog: catalog.clone(),
        input: input.clone(),
        contract,
        url,
        credential,
    }))
}
struct PreparedRead {
    catalog: DiscoveryCatalog,
    input: Value,
    contract: Value,
    url: Url,
    credential: Option<Zeroizing<String>>,
}
impl PreparedAdapterInvocation for PreparedRead {
    fn execute(self: Box<Self>) -> AdapterExecutionObservation {
        let mut output = json!({"ok":false,"error":"http_request_failed","collection":self.input["collection"],"tool":self.input["tool"],"contractDigest":self.input["contractDigest"],"artifact_digest":self.catalog.snapshot(self.input["collection"].as_str().expect("validated collection")),"backend_version":"0.46.0","execution_enabled":true,"request":self.input,"method":"GET","target":self.url.as_str(),"status":null,"body":null});
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .max_response_header_size(16 * 1024)
            .user_agent("xgen-http-read/1.0")
            .build()
            .new_agent();
        let mut request = agent
            .get(self.url.as_str())
            .header("Accept", "application/json");
        if let Some(secret) = &self.credential {
            request = request.header("Authorization", format!("Bearer {}", secret.as_str()));
        }
        if let Ok(mut response) = request.call() {
            let status = response.status().as_u16();
            output["status"] = json!(status);
            let json_media = response
                .headers()
                .get("Content-Type")
                .and_then(|header| header.to_str().ok())
                .is_some_and(|header| {
                    header
                        .split(';')
                        .next()
                        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
                });
            if (200..=299).contains(&status) && !json_media {
                output["error"] = json!("http_response_media_invalid");
            } else if (200..=299).contains(&status) {
                let mut bytes = Vec::new();
                if response
                    .body_mut()
                    .as_reader()
                    .take(u64::try_from(MAX_BODY + 1).expect("fixed bound"))
                    .read_to_end(&mut bytes)
                    .is_ok()
                    && bytes.len() <= MAX_BODY
                {
                    if self
                        .credential
                        .as_ref()
                        .is_some_and(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
                    {
                        output["error"] = json!("http_credential_echo");
                    } else if let Ok(body) = serde_json::from_slice::<Value>(&bytes) {
                        if self
                            .credential
                            .as_ref()
                            .is_some_and(|secret| contains_secret(&body, secret))
                        {
                            output["error"] = json!("http_credential_echo");
                        } else if response_valid(&self.contract, status, &body) {
                            output["ok"] = json!(true);
                            output["error"] = Value::Null;
                            output["body"] = body;
                        } else {
                            output["error"] = json!("http_response_schema_invalid");
                        }
                    } else {
                        output["error"] = json!("http_response_json_invalid");
                    }
                } else {
                    output["error"] = json!("http_response_size_or_io_limit");
                }
            } else {
                output["error"] = json!("http_status_rejected");
            }
        }
        if serde_jcs::to_vec(&output)
            .map_or(true, |bytes| bytes.len() > graph_discovery::MAX_OUTPUT)
        {
            output["ok"] = json!(false);
            output["body"] = Value::Null;
            output["error"] = json!("http_response_unverifiable");
        }
        let encoded = serde_jcs::to_vec(&output).expect("finite bounded output");
        AdapterExecutionObservation::SucceededWithOutput {
            evidence_digest: AdapterEvidenceDigest::new(digest(&encoded))
                .expect("canonical digest"),
            output: AdapterToolOutput::new(output),
        }
    }
}
fn contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(text) => text.contains(secret),
        Value::Array(items) => items.iter().any(|v| contains_secret(v, secret)),
        Value::Object(items) => items
            .iter()
            .any(|(k, v)| k.contains(secret) || contains_secret(v, secret)),
        _ => false,
    }
}

fn response_valid(contract: &Value, status: u16, body: &Value) -> bool {
    contract["responses"]
        .get(status.to_string())
        .is_some_and(|schema| {
            jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .offline()
                .build(schema)
                .is_ok_and(|validator| validator.is_valid(body))
        })
}

pub(crate) fn inspect(catalog: &DiscoveryCatalog, input: &Value, output: &Value) -> Result<(), ()> {
    let contract = descriptor(catalog, input).map_err(|_| ())?;
    let name = input["collection"].as_str().ok_or(())?;
    let connection = catalog.connection(name).ok_or(())?;
    let target = request_url(connection, &contract, input).map_err(|_| ())?;
    if output.as_object().is_none_or(|object| object.len() != 13)
        || output["method"] != "GET"
        || output["target"] != target.as_str()
        || output["tool"] != input["tool"]
        || output["contractDigest"] != input["contractDigest"]
        || !output["ok"].is_boolean()
        || !output["execution_enabled"].as_bool().unwrap_or(false)
    {
        return Err(());
    }
    if output["ok"] == true {
        let status = output["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .ok_or(())?;
        if !(200..=299).contains(&status)
            || !output["error"].is_null()
            || !response_valid(&contract, status, &output["body"])
        {
            return Err(());
        }
    } else if !output["body"].is_null()
        || !matches!(
            output["error"].as_str(),
            Some(
                "http_request_failed"
                    | "http_credential_echo"
                    | "http_response_schema_invalid"
                    | "http_response_json_invalid"
                    | "http_response_size_or_io_limit"
                    | "http_status_rejected"
                    | "http_response_unverifiable"
                    | "http_response_media_invalid"
            )
        )
        || (!output["status"].is_null()
            && !output["status"]
                .as_u64()
                .is_some_and(|status| (100..=599).contains(&status)))
    {
        return Err(());
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::from("sha256:");
    for b in Sha256::digest(bytes) {
        write!(&mut result, "{b:02x}").expect("String write");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        Connection {
            format_version: 1,
            base_url: "https://example.test/api".into(),
            allow_get: true,
            bearer_env: None,
            credential_ref: None,
        }
    }
    fn contract() -> Value {
        json!({"path":"/records/{id}","base_path":"","parameters":[
            {"name":"id","location":"path","required":true,"schema":{"type":"integer","minimum":1}},
            {"name":"filter","location":"query","required":false,"schema":{"type":"string","minLength":1}},
            {"name":"active","location":"query","required":false,"schema":{"type":"boolean"}}
        ],"responses":{"200":{"type":"object","required":["value"],"properties":{"value":{"type":"integer","minimum":0}},"additionalProperties":false}}})
    }
    #[test]
    fn url_uses_fixed_origin_and_typed_encoded_parameters() {
        let input =
            json!({"parameters":{"path":{"id":42},"query":{"active":true,"filter":"a&b 한글"}}});
        let url = request_url(&connection(), &contract(), &input).unwrap();
        assert_eq!(url.host_str(), Some("example.test"));
        assert_eq!(url.path(), "/api/records/42");
        assert_eq!(
            url.query_pairs()
                .collect::<BTreeMap<_, _>>()
                .get("filter")
                .unwrap(),
            "a&b 한글"
        );
        let mut swagger = contract();
        swagger["base_path"] = json!("/v1");
        assert_eq!(
            request_url(&connection(), &swagger, &input).unwrap().path(),
            "/api/v1/records/42"
        );
    }
    #[test]
    fn invalid_or_unknown_parameters_are_rejected_before_io() {
        for parameters in [
            json!({"path":{},"query":{}}),
            json!({"path":{"id":0},"query":{}}),
            json!({"path":{"id":"42"},"query":{}}),
            json!({"path":{"id":42},"query":{"filter":""}}),
            json!({"path":{"id":42},"query":{"unknown":"x"}}),
            json!({"path":{"id":42},"query":{"active":"true"}}),
        ] {
            assert!(
                request_url(
                    &connection(),
                    &contract(),
                    &json!({"parameters":parameters})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn path_arguments_cannot_change_origin_or_escape_prefix() {
        let mut schema = contract();
        schema["parameters"][0]["schema"] = json!({"type":"string"});
        for id in ["..", ".", "../other", "%2f", "a\\b", ""] {
            assert!(
                request_url(
                    &connection(),
                    &schema,
                    &json!({"parameters":{"path":{"id":id},"query":{}}})
                )
                .is_err()
            );
        }
        for path in ["//evil.test", "/../other", "/%2e%2e", "/a\\b"] {
            schema["path"] = json!(path);
            assert!(
                request_url(
                    &connection(),
                    &schema,
                    &json!({"parameters":{"path":{"id":"ok"},"query":{}}})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn connection_rejects_insecure_auth_or_ambiguous_targets() {
        assert!(connection().validate().is_ok());
        for base_url in [
            "http://example.test",
            "https://user:pass@example.test",
            "https://example.test?token=x",
            "https://example.test/#x",
            "file:///tmp/test",
            "https://example.test/%2e",
        ] {
            let mut c = connection();
            c.base_url = base_url.into();
            assert!(c.validate().is_err());
        }
        let mut c = connection();
        c.bearer_env = Some("TOKEN".into());
        c.credential_ref = Some(format!("cred-{}", "a".repeat(32)));
        assert!(c.validate().is_err());
        c.credential_ref = None;
        assert!(c.validate().is_ok());
        c.allow_get = false;
        assert!(c.validate().is_err());
    }
    #[test]
    fn response_schema_and_decoded_secret_echo_are_checked() {
        assert!(response_valid(&contract(), 200, &json!({"value":3})));
        for (status, body) in [
            (201, json!({"value":3})),
            (200, json!({"value":-1})),
            (200, json!({"value":"3"})),
            (200, json!({"other":3})),
        ] {
            assert!(!response_valid(&contract(), status, &body));
        }
        assert!(contains_secret(
            &json!({"data":["prefix fixture-credential suffix"]}),
            "fixture-credential"
        ));
        assert!(contains_secret(
            &json!({"fixture-credential":true}),
            "fixture-credential"
        ));
        assert!(!contains_secret(
            &json!({"data":"ordinary response"}),
            "fixture-credential"
        ));
    }
}
