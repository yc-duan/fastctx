//! fastshell core: bash-backed foreground commands and bounded background jobs.

mod apply_patch_hint;
pub(crate) mod bash;
mod buffer;
mod encoding;
mod foreground;
pub(crate) mod jobs;
mod normalize;
mod output;
mod process;

use crate::model::ToolResponse;
use crate::paths::{canonical_existing, display_path, parse_input_path};
use bash::BashLocator;
use jobs::JobManager;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 240_000;
const DEFAULT_WAIT_MS: u64 = 30_000;
const MAX_WAIT_MS: u64 = 60_000;

fn default_login_shell() -> bool {
    true
}

fn default_wait_ms() -> Option<u64> {
    Some(DEFAULT_WAIT_MS)
}

/// Parameters for a foreground bash command.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    /// The bash command line to run (passed to bash).
    pub command: String,
    /// Absolute path of the working directory. Omit for the session working directory.
    pub cwd: Option<String>,
    /// Kill the command (whole process tree) after this many milliseconds.
    #[schemars(range(min = 1, max = 240_000))]
    pub timeout_ms: Option<u64>,
    /// Run with a login shell (bash -lc) so your profile is loaded (PATH for nvm/cargo/pyenv).
    /// Set false for a clean non-login shell (--noprofile --norc) that skips profile side effects.
    #[serde(default = "default_login_shell")]
    #[schemars(default = "default_login_shell")]
    pub login_shell: bool,
    /// Known source encoding of the command's output, as a WHATWG label like "gbk" or
    /// "shift_jis". The response is always UTF-8. Omit for automatic detection.
    pub encoding: Option<String>,
}

/// Parameters for starting a background bash command.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunBackgroundRequest {
    /// The bash command line to run (passed to bash).
    pub command: String,
    /// Absolute path of the working directory. Omit for the session working directory.
    pub cwd: Option<String>,
    /// Same as run: login shell (bash -lc) by default; false for a clean non-login shell.
    #[serde(default = "default_login_shell")]
    #[schemars(default = "default_login_shell")]
    pub login_shell: bool,
    /// Default source encoding for this job's output when read with job_output (WHATWG label
    /// like "gbk"). Each job_output call may override it.
    pub encoding: Option<String>,
}

/// Parameters for querying a background job.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobOutputRequest {
    /// The job id returned by run_background.
    pub job_id: String,
    /// How long this query may take, in milliseconds. It returns earlier only when the job ends.
    /// Use 0 for an immediate snapshot.
    #[schemars(default = "default_wait_ms", range(min = 0, max = 60_000))]
    pub wait_ms: Option<u64>,
    /// Return output after this line number of the job's log. Omit to continue where your last
    /// call left off; pass it to re-read a stretch you already saw, for example with a different
    /// encoding.
    #[schemars(range(min = 0))]
    pub after_seq: Option<u64>,
    /// Decode this job's stored output with this source encoding for this call (WHATWG label
    /// like "gbk"). Overrides the job's default. The response is always UTF-8.
    pub encoding: Option<String>,
}

/// Parameters for terminating a background job tree.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobKillRequest {
    /// The job id returned by run_background.
    pub job_id: String,
}

/// Parameters for listing persistent background jobs.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobListRequest {
    /// Lifecycle subset to list. Omit for currently running jobs.
    #[serde(default)]
    #[schemars(default)]
    pub status: JobListStatus,
    /// Maximum records in this page. Omit to use `fastshell.job_list_limit` (default 20).
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<i64>,
    /// Skip this many entries of the sorted list (from a prior Partial note's offset).
    #[schemars(range(min = 0))]
    pub offset: Option<i64>,
}

/// Lifecycle subset exposed by `job_list`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobListStatus {
    /// Jobs whose process trees are still alive.
    #[default]
    Running,
    /// Retained exited and interrupted records.
    Finished,
    /// Running jobs followed by retained terminal records.
    All,
}

/// Stateful shell service shared by all five tools in one MCP server process.
#[derive(Clone, Debug)]
pub struct FastShell {
    bash: Arc<BashLocator>,
    jobs: JobManager,
}

impl FastShell {
    /// Creates a shell service whose background-job registry is shared across server restarts.
    pub fn new() -> Self {
        Self {
            bash: Arc::new(BashLocator::default()),
            jobs: JobManager::new(),
        }
    }

    /// Executes a bounded foreground command.
    pub fn run(&self, request: RunRequest) -> ToolResponse {
        self.run_until_cancelled(request, || false)
    }

    pub(crate) fn run_until_cancelled(
        &self,
        request: RunRequest,
        cancelled: impl Fn() -> bool,
    ) -> ToolResponse {
        if request.command.trim().is_empty() {
            return ToolResponse::error("Invalid command: it must be a non-empty string.");
        }
        let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return invalid_timeout(timeout_ms);
        }
        if let Err(error) = output::validate_run_budget(timeout_ms) {
            return ToolResponse::error(error);
        }
        let encoding = match request
            .encoding
            .as_deref()
            .map(encoding::validate_output_encoding)
            .transpose()
        {
            Ok(encoding) => encoding,
            Err(error) => return ToolResponse::error(error),
        };
        let cwd = match resolve_cwd(request.cwd.as_deref()) {
            Ok(cwd) => cwd,
            Err(error) => return ToolResponse::error(error),
        };
        let bash = match self.bash.resolve() {
            Ok(bash) => bash,
            Err(error) => return ToolResponse::error(error),
        };
        foreground::run(
            &bash,
            &request.command,
            &cwd,
            timeout_ms,
            request.login_shell,
            encoding,
            cancelled,
        )
    }

    /// Starts a background command and returns immediately.
    pub fn run_background(&self, request: RunBackgroundRequest) -> ToolResponse {
        if request.command.trim().is_empty() {
            return ToolResponse::error("Invalid command: it must be a non-empty string.");
        }
        let encoding = match request
            .encoding
            .as_deref()
            .map(encoding::validate_output_encoding)
            .transpose()
        {
            Ok(encoding) => encoding,
            Err(error) => return ToolResponse::error(error),
        };
        let cwd = match resolve_cwd(request.cwd.as_deref()) {
            Ok(cwd) => cwd,
            Err(error) => return ToolResponse::error(error),
        };
        let bash = match self.bash.resolve() {
            Ok(bash) => bash,
            Err(error) => return ToolResponse::error(error),
        };
        self.jobs
            .start(&bash, &request.command, &cwd, request.login_shell, encoding)
    }

    /// Returns output after an explicit sequence anchor or the server-side cursor.
    pub fn job_output(&self, request: JobOutputRequest) -> ToolResponse {
        self.job_output_until_cancelled(request, || false)
    }

    pub(crate) fn job_output_until_cancelled(
        &self,
        request: JobOutputRequest,
        cancelled: impl Fn() -> bool,
    ) -> ToolResponse {
        let wait_ms = request.wait_ms.unwrap_or(DEFAULT_WAIT_MS);
        if wait_ms > MAX_WAIT_MS {
            return ToolResponse::error(format!(
                "Invalid wait_ms value: {wait_ms}. Expected an integer from 0 to 60000."
            ));
        }
        let encoding = match request
            .encoding
            .as_deref()
            .map(encoding::validate_output_encoding)
            .transpose()
        {
            Ok(encoding) => encoding,
            Err(error) => return ToolResponse::error(error),
        };
        self.jobs.output_until_cancelled(
            &request.job_id,
            wait_ms,
            request.after_seq,
            encoding,
            cancelled,
        )
    }

    /// Terminates a job's whole process tree, or reports its prior exit.
    pub fn job_kill(&self, request: JobKillRequest) -> ToolResponse {
        self.jobs.kill(&request.job_id)
    }

    /// Lists a bounded lifecycle subset across every server session.
    pub fn job_list(&self, request: JobListRequest) -> ToolResponse {
        let offset = request.offset.unwrap_or(0);
        if offset < 0 {
            return ToolResponse::error(format!(
                "Invalid offset value: {offset}. Expected a non-negative integer."
            ));
        }
        if let Some(limit) = request.limit
            && !(1..=crate::control::settings::MAX_JOB_LIST_LIMIT as i64).contains(&limit)
        {
            return ToolResponse::error(format!(
                "Invalid limit value: {limit}. Expected an integer from 1 to 100."
            ));
        }
        self.jobs.list(
            request.status,
            offset as u64,
            request.limit.map(|limit| limit as u64),
        )
    }
}

impl Default for FastShell {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid_timeout(timeout_ms: u64) -> ToolResponse {
    ToolResponse::error(format!(
        "Invalid timeout_ms value: {timeout_ms}. Expected an integer from 1 to 240000."
    ))
}

fn resolve_cwd(input: Option<&str>) -> Result<PathBuf, String> {
    let path = match input {
        Some(input) => {
            let path = parse_input_path(input);
            if !path.is_absolute() {
                return Err("The cwd parameter must be an absolute path.".to_string());
            }
            if !path.exists() {
                return Err(format!(
                    "Working directory does not exist: {}",
                    display_path(&path)
                ));
            }
            if !path.is_dir() {
                return Err(format!(
                    "Working directory is not a directory: {}",
                    display_path(&path)
                ));
            }
            path
        }
        None => std::env::current_dir()
            .map_err(|error| format!("Cannot determine the session working directory: {error}."))?,
    };
    Ok(canonical_existing(&path).unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::{FastShell, JobOutputRequest, RunRequest};

    #[test]
    fn validation_errors_are_exact_and_do_not_probe_bash() {
        let shell = FastShell::new();
        assert_eq!(
            shell.run(RunRequest {
                command: " ".to_string(),
                cwd: None,
                timeout_ms: None,
                login_shell: true,
                encoding: None,
            }),
            crate::ToolResponse::error("Invalid command: it must be a non-empty string.")
        );
        assert_eq!(
            shell.job_output(JobOutputRequest {
                job_id: "missing".to_string(),
                wait_ms: Some(60_001),
                after_seq: None,
                encoding: None,
            }),
            crate::ToolResponse::error(
                "Invalid wait_ms value: 60001. Expected an integer from 0 to 60000."
            )
        );
    }
}
