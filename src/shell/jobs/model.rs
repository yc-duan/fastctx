//! On-disk background-job records and process-launch messages.

pub(crate) use crate::process_identity::ProcessIdentity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

pub(super) const JOB_SCHEMA_VERSION: u32 = 2;
pub(super) const META_FILE: &str = "meta.json";
pub(super) const EXIT_FILE: &str = "exit.json";
pub(super) const CAPTURE_ERROR_FILE: &str = "capture-error.json";
pub(super) const KILL_REQUEST_FILE: &str = "kill.request";

/// Best-effort provenance captured by the server that requested a background job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OriginSnapshot {
    pub(crate) server_pid: u32,
    /// Process creation token disambiguating a recycled server PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) server_started: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent_executable: Option<String>,
    pub(crate) server_cwd: String,
}

/// Immutable metadata written once after the detached supervisor owns the process tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct JobMeta {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) login_shell: bool,
    /// Default delivery-time decoder selected when this job was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) encoding: Option<String>,
    pub(crate) supervisor: ProcessIdentity,
    pub(crate) origin: OriginSnapshot,
    pub(crate) started_at: String,
    /// Nanosecond ordering key kept separate so the public timestamp remains second-precision.
    #[serde(default)]
    pub(crate) started_at_unix_nanos: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) isolation_warning: Option<String>,
}

/// Why the supervisor wrote the terminal record.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TerminationKind {
    #[default]
    Exited,
    Killed,
}

/// Immutable terminal record written exactly once after the process tree exits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExitRecord {
    pub(crate) exit_code: i32,
    pub(crate) total_lines: u64,
    pub(crate) had_loss: bool,
    pub(crate) ended_at: String,
    /// Nanosecond ordering key for deterministic newest-first listing and oldest-first reaping.
    #[serde(default)]
    pub(crate) ended_at_unix_nanos: u64,
    #[serde(default, skip_serializing_if = "is_natural_exit")]
    pub(crate) termination: TerminationKind,
    /// Fallback copy of a capture failure when its dedicated immutable record could not be published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capture_error: Option<CaptureErrorRecord>,
}

fn is_natural_exit(kind: &TerminationKind) -> bool {
    *kind == TerminationKind::Exited
}

/// One durable capture failure; the command itself continues to run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CaptureErrorRecord {
    pub(crate) after_seq: u64,
    pub(crate) reason: String,
}

/// One normalized display line in an append-only spool segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SpoolLine {
    pub(crate) seq: u64,
    /// UTF-8 text from schema v1 records, retained for backward compatibility.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) text: String,
    /// Raw normalized bytes written by schema v2 supervisors.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_base64"
    )]
    pub(crate) raw_bytes: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stream_encoding: Option<crate::shell::normalize::StreamEncoding>,
    pub(crate) truncated: bool,
    /// Lifetime loss flag propagated forward so retained segments remain self-describing.
    pub(crate) had_loss: bool,
}

/// Complete launch payload streamed over a private inherited pipe, never exposed in argv.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LaunchSpec {
    pub(crate) job_id: String,
    pub(crate) job_dir: PathBuf,
    pub(crate) bash: PathBuf,
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    pub(crate) login_shell: bool,
    #[serde(default)]
    pub(crate) encoding: Option<String>,
    pub(crate) origin: OriginSnapshot,
}

impl SpoolLine {
    pub(crate) fn encoded_line(&self) -> crate::shell::encoding::EncodedLine<'_> {
        match self.raw_bytes.as_deref() {
            Some(bytes) => crate::shell::encoding::EncodedLine {
                bytes,
                total_bytes: self.total_bytes.max(bytes.len() as u64),
                stream_encoding: self.stream_encoding,
                legacy_text: None,
                known_truncated: self.truncated,
            },
            None => crate::shell::encoding::EncodedLine {
                bytes: &[],
                total_bytes: self.total_bytes.max(self.text.len() as u64),
                stream_encoding: None,
                legacy_text: Some(&self.text),
                known_truncated: self.truncated,
            },
        }
    }
}

mod optional_base64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => {
                serializer.serialize_some(&base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|value| {
                base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum JobStatus {
    Running,
    Exited(ExitRecord),
    Interrupted,
}

impl JobStatus {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// One validated registry entry assembled independently from its job directory.
#[derive(Clone, Debug)]
pub(crate) struct JobRecord {
    pub(crate) id: String,
    pub(crate) directory: PathBuf,
    pub(crate) meta: JobMeta,
    pub(crate) status: JobStatus,
    pub(crate) ended_sort_key: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::JobMeta;

    #[test]
    fn version_one_metadata_without_new_origin_fields_remains_readable() {
        let source = r#"{
            "schema_version": 1,
            "command": "printf ok",
            "cwd": "/workspace",
            "login_shell": false,
            "supervisor": {"pid": 42, "started": "supervisor-token"},
            "origin": {
                "server_pid": 7,
                "parent_executable": "codex",
                "server_cwd": "/workspace"
            },
            "started_at": "2026-07-16T10:00:00Z"
        }"#;

        let meta: JobMeta = serde_json::from_str(source).unwrap();
        assert_eq!(meta.schema_version, 1);
        assert_eq!(meta.origin.server_pid, 7);
        assert_eq!(meta.origin.server_started, None);
        assert_eq!(meta.origin.parent_pid, None);
        assert_eq!(meta.encoding, None);
    }
}
