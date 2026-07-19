//! Versioned, machine-private update-check cache and last-attempt status.

use super::model::{CheckFailureKind, NpmDiscovery, NpmVersionAuthority};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 3;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
pub(crate) const SUCCESS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
static RECORD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub(crate) enum CachedOutcome {
    Current,
    GithubAvailable { target_version: String },
    NpmCurrent { discovery: NpmDiscovery },
    NpmAvailable { discovery: NpmDiscovery },
    NpmPending { discovery: NpmDiscovery },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
enum LastOutcome {
    Current,
    Available {
        target_version: String,
        source: Option<String>,
    },
    NpmPending {
        target_version: String,
        source: Option<String>,
        authority: NpmVersionAuthority,
    },
    Failed {
        kind: CheckFailureKind,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SuccessRecord {
    schema_version: u32,
    generation: String,
    current_version: String,
    checked_at_unix: u64,
    outcome: CachedOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LastRecord {
    schema_version: u32,
    generation: String,
    current_version: String,
    checked_at_unix: u64,
    outcome: LastOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckStatus {
    pub(crate) detail: String,
}

pub(crate) fn directory() -> PathBuf {
    crate::edit::private_storage::update_check_directory()
}

pub(crate) fn load_fresh_success(
    directory: &Path,
    channel_key: &str,
    current_version: &str,
    now: SystemTime,
) -> Option<CachedOutcome> {
    let record: SuccessRecord = read_record(&success_path(directory, channel_key))?;
    let last: LastRecord = read_record(&last_path(directory, channel_key))?;
    if record.schema_version != SCHEMA_VERSION || record.current_version != current_version {
        return None;
    }
    if last.schema_version != SCHEMA_VERSION
        || last.generation != record.generation
        || last.current_version != record.current_version
        || last.checked_at_unix != record.checked_at_unix
        || last.outcome != last_success(&record.outcome)
    {
        return None;
    }
    let checked_at = UNIX_EPOCH.checked_add(Duration::from_secs(record.checked_at_unix))?;
    let age = now.duration_since(checked_at).ok()?;
    (age <= SUCCESS_TTL).then_some(record.outcome)
}

pub(crate) fn record_success(
    directory: &Path,
    channel_key: &str,
    current_version: &str,
    checked_at: SystemTime,
    outcome: &CachedOutcome,
) -> Result<(), String> {
    let checked_at_unix = unix_seconds(checked_at)?;
    let generation = new_generation();
    let success = SuccessRecord {
        schema_version: SCHEMA_VERSION,
        generation: generation.clone(),
        current_version: current_version.to_string(),
        checked_at_unix,
        outcome: outcome.clone(),
    };
    let last = LastRecord {
        schema_version: SCHEMA_VERSION,
        generation,
        current_version: current_version.to_string(),
        checked_at_unix,
        outcome: last_success(outcome),
    };
    // The success record is the commit point. Readers require both records to carry the same
    // generation, so a crash or second-write failure becomes a cache miss instead of suppressing
    // the next network retry.
    write_record(directory, &last_path(directory, channel_key), &last)?;
    write_record(directory, &success_path(directory, channel_key), &success)
}

pub(crate) fn record_failure(
    directory: &Path,
    channel_key: &str,
    current_version: &str,
    checked_at: SystemTime,
    kind: CheckFailureKind,
    message: &str,
) -> Result<(), String> {
    let checked_at_unix = unix_seconds(checked_at)?;
    let last = LastRecord {
        schema_version: SCHEMA_VERSION,
        generation: new_generation(),
        current_version: current_version.to_string(),
        checked_at_unix,
        outcome: LastOutcome::Failed {
            kind,
            message: message.to_string(),
        },
    };
    write_record(directory, &last_path(directory, channel_key), &last)
}

pub(crate) fn invalidate_success(directory: &Path, channel_key: &str) {
    match fs::remove_file(success_path(directory, channel_key)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

pub(crate) fn status(directory: &Path, channel_key: &str, current_version: &str) -> CheckStatus {
    let Some(record): Option<LastRecord> = read_record(&last_path(directory, channel_key)) else {
        return CheckStatus {
            detail: "No update check has completed for this installation source.".to_string(),
        };
    };
    if record.schema_version != SCHEMA_VERSION || record.current_version != current_version {
        return CheckStatus {
            detail:
                "No update check has completed for this FastCtx version and installation source."
                    .to_string(),
        };
    }
    let timestamp = format_utc(record.checked_at_unix);
    let result = match record.outcome {
        LastOutcome::Current => format!("checked {timestamp}; v{current_version} is current"),
        LastOutcome::Available {
            target_version,
            source,
        } => {
            let source = source
                .map(|source| format!(" from {source}"))
                .unwrap_or_default();
            format!("checked {timestamp}; v{target_version} is available{source}")
        }
        LastOutcome::NpmPending {
            target_version,
            source,
            authority,
        } => format!(
            "checked {timestamp}; v{target_version} is known via {}, but {} is not yet complete",
            match authority {
                NpmVersionAuthority::Official => "official version authority",
                NpmVersionAuthority::MirrorFallback => {
                    "a mirror because both official channels were unavailable"
                }
            },
            source.unwrap_or_else(|| "the configured source set".to_string())
        ),
        LastOutcome::Failed { kind, message } => format!(
            "checked {timestamp}; {} failure: {message}",
            match kind {
                CheckFailureKind::Transient => "transient",
                CheckFailureKind::Structural => "structural",
            }
        ),
    };
    CheckStatus { detail: result }
}

fn last_success(outcome: &CachedOutcome) -> LastOutcome {
    match outcome {
        CachedOutcome::Current => LastOutcome::Current,
        CachedOutcome::GithubAvailable { target_version } => LastOutcome::Available {
            target_version: target_version.clone(),
            source: Some("GitHub Release".to_string()),
        },
        CachedOutcome::NpmCurrent { .. } => LastOutcome::Current,
        CachedOutcome::NpmAvailable { discovery } => LastOutcome::Available {
            target_version: discovery.target_version.clone(),
            source: discovery.selected_registry.clone(),
        },
        CachedOutcome::NpmPending { discovery } => LastOutcome::NpmPending {
            target_version: discovery.target_version.clone(),
            source: discovery.selected_registry.clone(),
            authority: discovery.authority,
        },
    }
}

fn success_path(directory: &Path, channel_key: &str) -> PathBuf {
    directory.join(format!("cache-{}.json", safe_key(channel_key)))
}

fn last_path(directory: &Path, channel_key: &str) -> PathBuf {
    directory.join(format!("last-{}.json", safe_key(channel_key)))
}

fn safe_key(channel_key: &str) -> String {
    channel_key
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'-' {
                char::from(byte.to_ascii_lowercase())
            } else {
                '-'
            }
        })
        .collect()
}

fn write_record(directory: &Path, path: &Path, value: &impl Serialize) -> Result<(), String> {
    crate::edit::private_storage::ensure_private_directory(directory, "update-check")?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Cannot serialize update-check state: {error}"))?;
    crate::control::transaction::atomic_replace(path, &bytes, Some(0o600), false)
}

fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES
    {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn unix_seconds(value: SystemTime) -> Result<u64, String> {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "Cannot cache an update check before the Unix epoch.".to_string())
}

fn new_generation() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = RECORD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

fn format_utc(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

// Howard Hinnant's civil-calendar transform, shifted from 1970-01-01.
fn civil_from_days(days_since_epoch: i64) -> (i64, u64, u64) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u64, day as u64)
}

#[cfg(test)]
mod tests {
    use super::{
        CachedOutcome, CheckFailureKind, SUCCESS_TTL, load_fresh_success, record_failure,
        record_success, status,
    };
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn success_cache_has_a_strict_twenty_four_hour_ttl_and_version_key() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let outcome = CachedOutcome::GithubAvailable {
            target_version: "0.2.0".to_string(),
        };
        record_success(temp.path(), "github-release", "0.1.0", checked_at, &outcome).unwrap();

        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                checked_at + SUCCESS_TTL
            ),
            Some(outcome)
        );
        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                checked_at + SUCCESS_TTL + Duration::from_secs(1)
            ),
            None
        );
        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.2.0",
                checked_at + Duration::from_secs(1)
            ),
            None
        );
    }

    #[test]
    fn corrupted_and_unknown_cache_records_are_misses() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("cache-github-release.json"), b"{broken").unwrap();
        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                UNIX_EPOCH + Duration::from_secs(1)
            ),
            None
        );
        std::fs::write(
            temp.path().join("cache-github-release.json"),
            br#"{"schema_version":999,"current_version":"0.1.0","checked_at_unix":1,"outcome":{"result":"current"}}"#,
        )
        .unwrap();
        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                UNIX_EPOCH + Duration::from_secs(2)
            ),
            None
        );
    }

    #[test]
    fn failures_are_status_records_but_never_success_cache_entries() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        record_failure(
            temp.path(),
            "npm-fastctx",
            "0.1.0",
            checked_at,
            CheckFailureKind::Transient,
            "HTTP 429",
        )
        .unwrap();
        assert_eq!(
            load_fresh_success(
                temp.path(),
                "npm-fastctx",
                "0.1.0",
                checked_at + Duration::from_secs(1)
            ),
            None
        );
        let detail = status(temp.path(), "npm-fastctx", "0.1.0").detail;
        assert!(detail.contains("2023-11-14 22:13:20 UTC"), "{detail}");
        assert!(detail.contains("transient failure: HTTP 429"), "{detail}");
    }

    #[test]
    fn a_newer_failure_invalidates_an_older_success_even_if_its_file_survives() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        record_success(
            temp.path(),
            "github-release",
            "0.1.0",
            checked_at,
            &CachedOutcome::Current,
        )
        .unwrap();

        record_failure(
            temp.path(),
            "github-release",
            "0.1.0",
            checked_at + Duration::from_secs(1),
            CheckFailureKind::Structural,
            "cannot persist the completed check",
        )
        .unwrap();

        assert_eq!(
            load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                checked_at + Duration::from_secs(2)
            ),
            None
        );
        let detail = status(temp.path(), "github-release", "0.1.0").detail;
        assert!(
            detail.contains("structural failure: cannot persist the completed check"),
            "{detail}"
        );
    }
}
