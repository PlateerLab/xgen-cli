use std::fs::{self, OpenOptions};
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use xgen_local_store::{RunStore, SqliteRunStore};
use xgen_provider_openai::{ModelCallObservation, TokenUsage};

use crate::composition::PublicRunError;
use crate::run_layout::{RunLayout, discover_state_root};

const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 8_192;

pub(crate) struct UsageStore(Connection);

impl UsageStore {
    pub(crate) fn open(path: &Path) -> Result<Self, ()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(file) => drop(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(()),
        }
        validate_file(path)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| ())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(1))
            .map_err(|_| ())?;
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
            PRAGMA max_page_count=16384;
            CREATE TABLE IF NOT EXISTS observations(call_id TEXT PRIMARY KEY, record_json TEXT NOT NULL);").map_err(|_| ())?;
        Ok(Self(connection))
    }

    pub(crate) fn record(&mut self, observation: &ModelCallObservation) -> Result<(), ()> {
        let json = serde_json::to_string(observation).map_err(|_| ())?;
        if json.len() > 4_096 {
            return Err(());
        }
        self.0
            .execute(
                "INSERT INTO observations(call_id, record_json) VALUES (?1, ?2)",
                params![observation.call_id, json],
            )
            .map_err(|_| ())?;
        Ok(())
    }
}

fn validate_file(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > MAX_DATABASE_BYTES {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(());
        }
    }
    Ok(())
}

fn read_observations(path: &Path) -> Result<Vec<ModelCallObservation>, ()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(()),
        Ok(_) => {}
    }
    validate_file(path)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| ())?;
    let mut statement = connection
        .prepare("SELECT record_json FROM observations ORDER BY rowid LIMIT 8193")
        .map_err(|_| ())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    let mut observations = Vec::new();
    for row in rows {
        let json = row.map_err(|_| ())?;
        if observations.len() == MAX_OBSERVATIONS || json.len() > 4_096 {
            return Err(());
        }
        observations.push(serde_json::from_str(&json).map_err(|_| ())?);
    }
    Ok(observations)
}

/// Observational subtotals; incomplete records never imply zero usage or a refund.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub run_id: String,
    pub reserved_calls: u32,
    pub observed_calls: usize,
    pub unobserved_reservations: usize,
    pub calls_with_token_usage: usize,
    pub calls_with_cache_usage: usize,
    pub elapsed_millis_subtotal: u64,
    pub token_subtotal: TokenUsage,
    pub complete_token_usage: bool,
    pub cost_estimate: Option<CostEstimate>,
    pub calls: Vec<ModelCallObservation>,
}

/// Explicit USD price for one million tokens, with at most nine decimal places.
#[derive(Debug, Clone, Copy)]
pub struct UsdPerMillion(u64);

impl std::str::FromStr for UsdPerMillion {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = "expected a non-negative USD price with at most nine decimal places";
        if value.len() > 30 {
            return Err(invalid);
        }
        let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || fractional.len() > 9
            || !fractional.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid);
        }
        let whole: u64 = whole.parse().map_err(|_| invalid)?;
        let fraction = if fractional.is_empty() {
            0
        } else {
            fractional.parse::<u64>().map_err(|_| invalid)?
        };
        let scale = 10u64.pow(9 - u32::try_from(fractional.len()).map_err(|_| invalid)?);
        whole
            .checked_mul(1_000_000_000)
            .and_then(|whole| whole.checked_add(fraction * scale))
            .map(Self)
            .ok_or(invalid)
    }
}

/// Caller-supplied prices; endpoint/model names never select an implicit price table.
#[derive(Debug, Clone, Copy)]
pub struct TokenPrices {
    pub input: UsdPerMillion,
    pub cached_input: UsdPerMillion,
    pub output: UsdPerMillion,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    pub currency: &'static str,
    pub estimated_usd_subtotal: String,
    pub priced_calls: usize,
    pub partial: bool,
    pub input_usd_per_million: String,
    pub cached_input_usd_per_million: String,
    pub output_usd_per_million: String,
}

impl UsageReport {
    /// Estimate only calls with known token/cache counts using explicit caller-supplied prices.
    #[must_use]
    pub fn with_prices(mut self, prices: TokenPrices) -> Self {
        self.cost_estimate = None;
        let mut numerator = 0u128;
        let mut priced_calls = 0;
        for call in &self.calls {
            let Some(usage) = &call.usage else {
                continue;
            };
            let Some(cached) = usage.cached_input_tokens else {
                continue;
            };
            let Some(uncached) = usage.input_tokens.checked_sub(cached) else {
                return self;
            };
            let Some(amount) = (u128::from(uncached) * u128::from(prices.input.0))
                .checked_add(u128::from(cached) * u128::from(prices.cached_input.0))
                .and_then(|value| {
                    value.checked_add(u128::from(usage.output_tokens) * u128::from(prices.output.0))
                })
            else {
                return self;
            };
            let Some(total) = numerator.checked_add(amount) else {
                return self;
            };
            numerator = total;
            priced_calls += 1;
        }
        if priced_calls > 0 {
            self.cost_estimate = Some(CostEstimate {
                currency: "USD",
                estimated_usd_subtotal: format_usd(numerator / 1_000_000),
                priced_calls,
                partial: priced_calls != self.reserved_calls as usize,
                input_usd_per_million: format_usd(u128::from(prices.input.0)),
                cached_input_usd_per_million: format_usd(u128::from(prices.cached_input.0)),
                output_usd_per_million: format_usd(u128::from(prices.output.0)),
            });
        }
        self
    }
}

fn format_usd(nano_usd: u128) -> String {
    format!(
        "{}.{:09}",
        nano_usd / 1_000_000_000,
        nano_usd % 1_000_000_000
    )
}

/// Inspect local usage without credentials, network requests, or changes to Run state.
///
/// # Errors
/// Returns a fixed configuration/integrity failure for missing or unreadable state.
pub fn inspect_local_usage(run_id: &str) -> Result<UsageReport, PublicRunError> {
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
    let reserved = state
        .agent_loop
        .as_ref()
        .and_then(|agent| agent.model_calls.as_ref())
        .map_or(0, |calls| calls.reserved_calls);
    let observations =
        read_observations(&layout.usage_path()).map_err(|()| PublicRunError::Integrity)?;
    summarize(run_id, reserved, observations).ok_or(PublicRunError::Integrity)
}

fn summarize(
    run_id: &str,
    reserved_calls: u32,
    calls: Vec<ModelCallObservation>,
) -> Option<UsageReport> {
    if calls.len() > reserved_calls as usize {
        return None;
    }
    let mut subtotal = TokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cached_input_tokens: Some(0),
        reasoning_tokens: Some(0),
    };
    let mut elapsed = 0u64;
    let mut known = 0;
    let mut cache_known = 0;
    for observation in &calls {
        elapsed = elapsed.checked_add(observation.elapsed_millis)?;
        if let Some(usage) = &observation.usage {
            if usage.input_tokens.checked_add(usage.output_tokens)? != usage.total_tokens
                || usage
                    .cached_input_tokens
                    .is_some_and(|count| count > usage.input_tokens)
                || usage
                    .reasoning_tokens
                    .is_some_and(|count| count > usage.output_tokens)
            {
                return None;
            }
            known += 1;
            cache_known += usize::from(usage.cached_input_tokens.is_some());
            subtotal.input_tokens = subtotal.input_tokens.checked_add(usage.input_tokens)?;
            subtotal.output_tokens = subtotal.output_tokens.checked_add(usage.output_tokens)?;
            subtotal.total_tokens = subtotal.total_tokens.checked_add(usage.total_tokens)?;
            subtotal.cached_input_tokens =
                sum_optional(subtotal.cached_input_tokens, usage.cached_input_tokens).ok()?;
            subtotal.reasoning_tokens =
                sum_optional(subtotal.reasoning_tokens, usage.reasoning_tokens).ok()?;
        } else {
            subtotal.cached_input_tokens = None;
            subtotal.reasoning_tokens = None;
        }
    }
    let missing = reserved_calls as usize - calls.len();
    if missing > 0 {
        subtotal.cached_input_tokens = None;
        subtotal.reasoning_tokens = None;
    }
    Some(UsageReport {
        run_id: run_id.to_owned(),
        reserved_calls,
        observed_calls: calls.len(),
        unobserved_reservations: missing,
        calls_with_token_usage: known,
        calls_with_cache_usage: cache_known,
        elapsed_millis_subtotal: elapsed,
        token_subtotal: subtotal,
        complete_token_usage: missing == 0 && known == calls.len(),
        cost_estimate: None,
        calls,
    })
}

fn sum_optional(a: Option<u64>, b: Option<u64>) -> Result<Option<u64>, ()> {
    match (a, b) {
        (Some(a), Some(b)) => a.checked_add(b).map(Some).ok_or(()),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgen_provider_openai::ModelCallOutcome;

    fn observation(id: &str) -> ModelCallObservation {
        ModelCallObservation {
            call_id: id.to_owned(),
            request_digest: "sha256:test".to_owned(),
            elapsed_millis: 3,
            outcome: ModelCallOutcome::ResponseRejected,
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                total_tokens: 12,
                cached_input_tokens: Some(4),
                reasoning_tokens: None,
            }),
        }
    }

    #[test]
    fn rejected_calls_survive_reopen_and_duplicate_ids_are_not_recounted() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("usage.sqlite3");
        let mut store = UsageStore::open(&path).unwrap();
        store.record(&observation("a")).unwrap();
        assert!(store.record(&observation("a")).is_err());
        drop(store);
        let mut store = UsageStore::open(&path).unwrap();
        store.record(&observation("b")).unwrap();
        let report = summarize("run", 2, read_observations(&path).unwrap()).unwrap();
        assert_eq!(report.observed_calls, 2);
        assert_eq!(report.token_subtotal.total_tokens, 24);
        assert_eq!(report.token_subtotal.cached_input_tokens, Some(8));
        assert!(report.complete_token_usage);
    }

    #[test]
    fn missing_records_and_unknown_cache_are_explicit() {
        let mut call = observation("a");
        call.usage.as_mut().unwrap().cached_input_tokens = None;
        let report = summarize("run", 2, vec![call]).unwrap();
        assert!(!report.complete_token_usage);
        assert_eq!(report.unobserved_reservations, 1);
        assert_eq!(report.token_subtotal.cached_input_tokens, None);
        let old = summarize("run", 3, vec![]).unwrap();
        assert_eq!(old.calls_with_token_usage, 0);
        assert!(!old.complete_token_usage);
    }

    #[test]
    fn cost_uses_cached_input_once_and_marks_incomplete_coverage() {
        let mut call = observation("a");
        call.usage = Some(TokenUsage {
            input_tokens: 10_000,
            output_tokens: 2_000,
            total_tokens: 12_000,
            cached_input_tokens: Some(6_000),
            reasoning_tokens: Some(500),
        });
        let prices = TokenPrices {
            input: "1".parse().unwrap(),
            cached_input: "0.1".parse().unwrap(),
            output: "2".parse().unwrap(),
        };
        let report = summarize("run", 2, vec![call]).unwrap().with_prices(prices);
        let estimate = report.cost_estimate.unwrap();
        assert_eq!(estimate.estimated_usd_subtotal, "0.008600000");
        assert!(estimate.partial);
        assert_eq!(estimate.priced_calls, 1);
        for invalid in [
            "-1",
            "NaN",
            "inf",
            "1e3",
            "1.0000000001",
            "18446744073709551615",
        ] {
            assert!(invalid.parse::<UsdPerMillion>().is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_symlinks_never_write_to_an_external_file() {
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("target");
        fs::write(&target, "untouched").unwrap();
        let path = fixture.path().join("usage.sqlite3");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(UsageStore::open(&path).is_err());
        assert!(read_observations(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }
}
