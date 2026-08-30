//! Persistent background jobs whose supervisors and records outlive every MCP server session.

pub(crate) mod admission;
mod background;
mod host;
mod identity;
mod model;
mod output_log;
mod store;

use crate::budget::{
    GLOBAL_TOKEN_BUDGET_ENV, JOB_OUTPUT_TOKEN_BUDGET_ENV, TokenBudget, estimate_tokens,
    relax_tool_token_budget, tool_token_budget_for_required,
};
use crate::control::paths::ControlPaths;
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::paths::display_path;
use crate::shell::JobListStatus;
use crate::shell::encoding::{
    OutputEncoding, decode_job, job_garble_note, validate_output_encoding,
};
use crate::shell::output::{
    budget_too_small_message, global_token_budget, job_output_token_budget,
};
use model::{JobRecord, JobStatus, LaunchSpec, StoredLine};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const KILL_ACK_TIMEOUT: Duration = Duration::from_secs(6);
const REGISTRY_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub(crate) struct JobManager {
    paths: Result<ControlPaths, String>,
    executable: Result<PathBuf, String>,
    admission_generation: Result<u64, String>,
    cursors: Arc<Mutex<HashMap<String, u64>>>,
    background: background::BackgroundTracker,
}

pub(crate) struct BackgroundLaunch<'a> {
    pub(crate) bash: &'a Path,
    pub(crate) command: &'a str,
    pub(crate) cwd: &'a Path,
    pub(crate) login_shell: bool,
    pub(crate) encoding: Option<OutputEncoding>,
    pub(crate) environment: &'a crate::session::SessionEnvironment,
    pub(crate) utf8_locale: &'a str,
}

#[derive(Clone, Debug)]
struct OutputSnapshot {
    status: JobStatus,
    head: Vec<StoredLine>,
    tail: Vec<StoredLine>,
    unread_first: u64,
    unread_last: u64,
    all_unread_loaded: bool,
    total_lines: u64,
    legacy_loss: bool,
    capture_error: Option<model::CaptureErrorRecord>,
    output_truncation: Option<model::OutputTruncationRecord>,
    default_encoding: Option<OutputEncoding>,
    anchor: u64,
    direct_log: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct FormattedPage {
    response: String,
    cursor_seq: Option<u64>,
}

/// Stable control-plane view of one persistent job record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobSummary {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) started_at: String,
    pub(crate) status: JobSummaryStatus,
    pub(crate) source: JobSourceSummary,
}

/// Stable best-effort source identity for grouping jobs from distinct server sessions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JobSourceSummary {
    pub(crate) key: String,
    pub(crate) tag: String,
    pub(crate) server_pid: u32,
    pub(crate) parent_executable: Option<String>,
    pub(crate) server_cwd: String,
}

/// Public three-state lifecycle used by CLI and TUI without exposing storage internals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobSummaryStatus {
    Running,
    Exited(i32),
    Interrupted,
}

/// Diagnosable registry failure with a stable permission classification for control surfaces.
#[derive(Debug)]
pub(crate) struct JobRegistryError {
    message: String,
    permission_denied: bool,
}

impl JobRegistryError {
    pub(super) fn from_io(context: String, error: std::io::Error) -> Self {
        Self {
            message: format!("{context}: {error}"),
            permission_denied: error.kind() == std::io::ErrorKind::PermissionDenied,
        }
    }

    pub(super) fn data(message: String) -> Self {
        Self {
            message,
            permission_denied: false,
        }
    }

    pub(crate) const fn is_permission_denied(&self) -> bool {
        self.permission_denied
    }
}

impl Display for JobRegistryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JobRegistryError {}

impl From<JobRegistryError> for String {
    fn from(error: JobRegistryError) -> Self {
        error.message
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KillState {
    Killed,
    AlreadyExited(i32),
    AlreadyInterrupted,
}

/// Read-only output tail for the TUI detail panel.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct JobTail {
    pub(crate) lines: Vec<String>,
    pub(crate) capture_error: Option<String>,
    pub(crate) output_truncation: Option<String>,
    cursor: TailCursor,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TailCursor {
    offsets: BTreeMap<PathBuf, u64>,
    direct_byte_offset: u64,
    last_seq: u64,
}

impl JobManager {
    pub(crate) fn new() -> Self {
        Self::with_session(crate::session::SessionContext::library_default())
    }

    pub(crate) fn with_session(session: Arc<crate::session::SessionContext>) -> Self {
        let paths = Ok(session.control_paths.clone());
        let admission_generation = paths
            .as_ref()
            .map_err(Clone::clone)
            .and_then(admission::observe_generation);
        Self {
            paths,
            executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}")),
            admission_generation,
            cursors: Arc::new(Mutex::new(HashMap::new())),
            background: background::BackgroundTracker::default(),
        }
    }

    pub(crate) fn start(&self, launch: BackgroundLaunch<'_>) -> ToolResponse {
        let BackgroundLaunch {
            bash,
            command,
            cwd,
            login_shell,
            encoding,
            environment,
            utf8_locale,
        } = launch;
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let executable = match self.executable.as_ref() {
            Ok(executable) => executable,
            Err(error) => return ToolResponse::error(error.clone()),
        };
        let admission_generation = match self.admission_generation.as_ref() {
            Ok(generation) => *generation,
            Err(error) => return ToolResponse::error(error.clone()),
        };
        let _admission = match admission::AdmissionGuard::acquire(paths) {
            Ok(guard) if guard.generation() == admission_generation => guard,
            Ok(_) => {
                return ToolResponse::error(
                    "This FastCtx server predates the most recent Unapply. Start a new ChatGPT/Codex session and retry run_background."
                        .to_string(),
                );
            }
            Err(error) => return ToolResponse::error(error),
        };
        let limits = match store::effective_limits(paths) {
            Ok(limits) => limits,
            Err(error) => return ToolResponse::error(error),
        };
        if let Err(error) = store::reap(paths, limits.storage_limit_mib) {
            return ToolResponse::error(error);
        }
        let (job_id, job_dir) = match store::reserve_job(&paths.jobs_dir) {
            Ok(reservation) => reservation,
            Err(error) => return ToolResponse::error(error),
        };
        let registry = match store::scan_registry(&paths.jobs_dir) {
            Ok(registry) => registry,
            Err(error) => {
                store::remove_reserved_job(&job_dir);
                return ToolResponse::error(error);
            }
        };
        let active = registry
            .records
            .iter()
            .filter(|record| record.status.is_running())
            .count() as u64
            + registry.pending_reservations;
        if active > limits.max_running_jobs {
            store::remove_reserved_job(&job_dir);
            return ToolResponse::error(format!(
                "Too many running jobs: the limit is {} across all FastCtx sessions for the current user. Kill or wait out an existing job first.",
                limits.max_running_jobs
            ));
        }

        let log_path = job_dir.join(model::OUTPUT_LOG_FILE);
        let response = HeadNote::new(format!("job {job_id}"), HeadMetric::event("started"))
            .fact(format!("log at {}", display_path(&log_path)))
            .render();
        let budget = match tool_token_budget_for_required(
            GLOBAL_TOKEN_BUDGET_ENV,
            estimate_tokens(&response),
        ) {
            Ok(budget) => budget,
            Err(error) => {
                store::remove_reserved_job(&job_dir);
                return ToolResponse::error(error);
            }
        };
        if estimate_tokens(&response) > budget.value {
            store::remove_reserved_job(&job_dir);
            return ToolResponse::error(budget_too_small_message(budget));
        }
        let spec = LaunchSpec {
            job_id: job_id.clone(),
            job_dir: job_dir.clone(),
            bash: bash.to_path_buf(),
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            login_shell,
            encoding: encoding.map(|encoding| encoding.label().to_string()),
            environment: environment.clone(),
            utf8_locale: utf8_locale.to_string(),
            output_limit_bytes: limits.storage_limit_mib.saturating_mul(1024 * 1024),
            origin: store::origin_snapshot(environment.cwd()),
        };
        match host::launch_supervisor(executable, &spec) {
            Ok(()) => {
                self.background.track_id(&job_id, SystemTime::now());
                ToolResponse::text(response)
            }
            Err(error) => {
                let live = store::read_json::<model::JobMeta>(
                    &job_dir.join(model::META_FILE),
                    "job metadata",
                )
                .ok()
                .flatten()
                .is_some_and(|meta| identity::identity_is_alive(&meta.supervisor));
                if !live {
                    store::remove_reserved_job(&job_dir);
                }
                ToolResponse::error(error)
            }
        }
    }

    pub(crate) fn output_until_cancelled(
        &self,
        job_id: &str,
        wait_ms: u64,
        after_seq: Option<u64>,
        encoding: Option<OutputEncoding>,
        cancelled: impl Fn() -> bool,
    ) -> ToolResponse {
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let mut budget = match job_output_token_budget() {
            Ok(budget) => budget,
            Err(error) => return ToolResponse::error(error),
        };
        let started = Instant::now();
        let wait = Duration::from_millis(wait_ms);
        let anchor = after_seq.unwrap_or_else(|| {
            self.cursors
                .lock()
                .unwrap()
                .get(job_id)
                .copied()
                .unwrap_or(0)
        });
        let record = loop {
            if cancelled() {
                return ToolResponse::error(
                    "The job output wait was cancelled because the MCP request or server session ended."
                        .to_string(),
                );
            }
            let record = match store::find_record(&paths.jobs_dir, job_id) {
                Ok(Some(record)) => {
                    self.background.track_record(&record, SystemTime::now());
                    record
                }
                Ok(None) => {
                    self.background.remove(job_id);
                    return missing_job(job_id);
                }
                Err(error) => return ToolResponse::error(error),
            };
            let capture_failed = match store::capture_error(&record) {
                Ok(capture_error) => capture_error.is_some(),
                Err(error) => return ToolResponse::error(error),
            };
            let output_truncated = match store::output_truncation(&record) {
                Ok(truncation) => truncation.is_some(),
                Err(error) => return ToolResponse::error(error),
            };
            if !record.status.is_running()
                || capture_failed
                || output_truncated
                || started.elapsed() >= wait
            {
                break record;
            }
            let remaining = wait.saturating_sub(started.elapsed());
            std::thread::sleep(remaining.min(REGISTRY_POLL));
        };
        let default_encoding = match record
            .meta
            .encoding
            .as_deref()
            .map(validate_output_encoding)
            .transpose()
        {
            Ok(encoding) => encoding,
            Err(error) => {
                return ToolResponse::error(format!(
                    "Cannot read job {job_id}: its stored output encoding is invalid ({error})"
                ));
            }
        };
        let page = loop {
            let snapshot = match load_output_snapshot(&record, anchor, default_encoding, budget) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    if error.to_ascii_lowercase().contains("too small to return") {
                        match relax_tool_token_budget(JOB_OUTPUT_TOKEN_BUDGET_ENV) {
                            Ok(Some(expanded)) => {
                                budget = expanded;
                                continue;
                            }
                            Ok(None) => {}
                            Err(config_error) => return ToolResponse::error(config_error),
                        }
                    }
                    return ToolResponse::error(error);
                }
            };
            match format_snapshot(job_id, wait_ms, &snapshot, encoding, budget) {
                Ok(page) => break page,
                Err(error) => {
                    if error.to_ascii_lowercase().contains("too small to return") {
                        match relax_tool_token_budget(JOB_OUTPUT_TOKEN_BUDGET_ENV) {
                            Ok(Some(expanded)) => {
                                budget = expanded;
                                continue;
                            }
                            Ok(None) => {}
                            Err(config_error) => return ToolResponse::error(config_error),
                        }
                    }
                    return ToolResponse::error(error);
                }
            }
        };
        if let Some(cursor_seq) = page.cursor_seq {
            let mut cursors = self.cursors.lock().unwrap();
            let cursor = cursors.entry(job_id.to_string()).or_insert(0);
            *cursor = (*cursor).max(cursor_seq);
        }
        if !record.status.is_running() {
            self.background.remove(job_id);
        }
        ToolResponse::text(page.response)
    }

    pub(crate) fn kill(&self, job_id: &str) -> ToolResponse {
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let killed = job_event(job_id, "killed");
        let required = [
            estimate_tokens(&killed),
            estimate_tokens(&job_event(job_id, &format!("already exited {}", i32::MIN))),
            estimate_tokens(&job_event(job_id, "already interrupted")),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        let budget = match tool_token_budget_for_required(GLOBAL_TOKEN_BUDGET_ENV, required) {
            Ok(budget) => budget,
            Err(error) => return ToolResponse::error(error),
        };
        if estimate_tokens(&killed) > budget.value {
            return ToolResponse::error(budget_too_small_message(budget));
        }
        let response = match terminate(paths, job_id) {
            Ok(KillState::Killed) => ToolResponse::text(killed),
            Ok(KillState::AlreadyExited(code)) => {
                global_head(job_event(job_id, &format!("already exited {code}")))
            }
            Ok(KillState::AlreadyInterrupted) => {
                global_head(job_event(job_id, "already interrupted"))
            }
            Err(error) => {
                if matches!(store::find_record(&paths.jobs_dir, job_id), Ok(None)) {
                    self.background.remove(job_id);
                }
                return ToolResponse::error(error);
            }
        };
        if !response.is_error {
            self.background.remove(job_id);
        }
        response
    }

    pub(crate) fn list(
        &self,
        status: JobListStatus,
        offset: u64,
        limit: Option<u64>,
    ) -> ToolResponse {
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let registry = match store::scan_registry(&paths.jobs_dir) {
            Ok(registry) => registry,
            Err(error) => return ToolResponse::error(error),
        };
        let limit = match limit {
            Some(limit) => limit,
            None => match crate::control::settings::load(paths) {
                Ok(settings) => settings.fastshell.job_list_limit,
                Err(error) => return ToolResponse::error(error),
            },
        };
        format_job_list(registry.records, status, offset, limit)
    }

    fn paths(&self) -> Result<&ControlPaths, String> {
        self.paths.as_ref().map_err(Clone::clone)
    }

    pub(crate) fn background_status_at(
        &self,
        exclude: Option<&str>,
        now: SystemTime,
    ) -> Option<crate::background_status::BackgroundStatus> {
        if !self.background.has_candidates(exclude) {
            return None;
        }
        let paths = self.paths().ok()?;
        self.background.snapshot(paths, exclude, now)
    }
}

fn terminate(paths: &ControlPaths, job_id: &str) -> Result<KillState, String> {
    let record =
        store::find_record(&paths.jobs_dir, job_id)?.ok_or_else(|| missing_job_text(job_id))?;
    match &record.status {
        JobStatus::Exited(exit) => return Ok(KillState::AlreadyExited(exit.exit_code)),
        JobStatus::Interrupted => return Ok(KillState::AlreadyInterrupted),
        JobStatus::Running => {}
    }
    store::request_kill(&record)?;
    let deadline = Instant::now() + KILL_ACK_TIMEOUT;
    loop {
        let record =
            store::find_record(&paths.jobs_dir, job_id)?.ok_or_else(|| missing_job_text(job_id))?;
        match record.status {
            JobStatus::Running if Instant::now() < deadline => {}
            JobStatus::Running => {
                return Err(format!(
                    "Cannot kill job {job_id}: its supervisor did not acknowledge within 6 seconds. Retry job_kill or stop the supervisor process manually."
                ));
            }
            JobStatus::Exited(exit) if exit.was_killed() => {
                return Ok(KillState::Killed);
            }
            JobStatus::Exited(exit) => return Ok(KillState::AlreadyExited(exit.exit_code)),
            JobStatus::Interrupted => return Ok(KillState::AlreadyInterrupted),
        }
        std::thread::sleep(REGISTRY_POLL);
    }
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

fn format_snapshot(
    job_id: &str,
    wait_ms: u64,
    snapshot: &OutputSnapshot,
    call_encoding: Option<OutputEncoding>,
    budget: TokenBudget,
) -> Result<FormattedPage, String> {
    if snapshot.head.is_empty() && snapshot.tail.is_empty() {
        let candidate = render_candidate(job_id, wait_ms, snapshot, call_encoding, 0, 0);
        if estimate_tokens(&candidate.response) > budget.value {
            return Err(budget_too_small_message(budget));
        }
        return Ok(FormattedPage {
            response: candidate.response,
            cursor_seq: (snapshot.unread_last > snapshot.anchor).then_some(snapshot.unread_last),
        });
    }

    if snapshot.all_unread_loaded {
        let candidate = render_candidate(
            job_id,
            wait_ms,
            snapshot,
            call_encoding,
            snapshot.head.len(),
            0,
        );
        if estimate_tokens(&candidate.response) <= budget.value {
            return Ok(FormattedPage {
                response: candidate.response,
                cursor_seq: snapshot
                    .direct_log
                    .as_ref()
                    .map(|_| snapshot.unread_last)
                    .or(candidate.last_seq),
            });
        }
    }

    if snapshot.direct_log.is_none() {
        return format_legacy_page(job_id, wait_ms, snapshot, call_encoding, budget);
    }

    format_direct_window(job_id, wait_ms, snapshot, call_encoding, budget)
}

#[derive(Debug)]
struct RenderedCandidate {
    response: String,
    last_seq: Option<u64>,
}

fn load_output_snapshot(
    record: &JobRecord,
    anchor: u64,
    default_encoding: Option<OutputEncoding>,
    budget: TokenBudget,
) -> Result<OutputSnapshot, String> {
    let mut log = store::open_log(record)?;
    let direct_log = log.direct_path().map(Path::to_path_buf);
    let total_lines = log.total_lines();
    let requested_first = anchor.saturating_add(1);
    let unread_first = requested_first.max(log.oldest_seq());
    let max_lines = budget.value.saturating_mul(4).saturating_add(64);
    let max_bytes = budget.value.saturating_mul(16).saturating_add(64 * 1024);
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut all_unread_loaded = true;
    if unread_first <= total_lines {
        let prefix = log.read_prefix_bounded(unread_first, total_lines, max_lines, max_bytes)?;
        all_unread_loaded = prefix.complete;
        head = prefix.lines;
        if !all_unread_loaded && direct_log.is_some() {
            if anchor != 0 {
                head.clear();
            }
            let suffix =
                log.read_suffix_bounded(unread_first, total_lines, max_lines, max_bytes)?;
            tail = suffix.lines;
            if let Some(last_head) = head.last().map(|line| line.seq) {
                tail.retain(|line| line.seq > last_head);
            }
        }
    }
    let legacy_loss = log.had_irretrievable_loss() || unread_first > requested_first;
    Ok(OutputSnapshot {
        status: record.status.clone(),
        head,
        tail,
        unread_first,
        unread_last: total_lines,
        all_unread_loaded,
        total_lines,
        legacy_loss,
        capture_error: log.capture_error.clone(),
        output_truncation: log.output_truncation.clone(),
        default_encoding,
        anchor,
        direct_log,
    })
}

fn format_legacy_page(
    job_id: &str,
    wait_ms: u64,
    snapshot: &OutputSnapshot,
    call_encoding: Option<OutputEncoding>,
    budget: TokenBudget,
) -> Result<FormattedPage, String> {
    let mut low = 1_usize;
    let mut high = snapshot.head.len();
    let mut best = None;
    while low <= high {
        let shown = low + (high - low) / 2;
        let candidate = render_candidate(job_id, wait_ms, snapshot, call_encoding, shown, 0);
        if estimate_tokens(&candidate.response) <= budget.value {
            best = Some(candidate);
            low = shown.saturating_add(1);
        } else if shown == 1 {
            break;
        } else {
            high = shown - 1;
        }
    }
    let candidate = best.ok_or_else(|| budget_too_small_message(budget))?;
    Ok(FormattedPage {
        response: candidate.response,
        cursor_seq: candidate.last_seq,
    })
}

fn format_direct_window(
    job_id: &str,
    wait_ms: u64,
    snapshot: &OutputSnapshot,
    call_encoding: Option<OutputEncoding>,
    budget: TokenBudget,
) -> Result<FormattedPage, String> {
    let tail_available = if snapshot.all_unread_loaded {
        snapshot.head.len()
    } else {
        snapshot.tail.len()
    };
    if tail_available == 0 {
        return Err(budget_too_small_message(budget));
    }
    let head_available = if snapshot.anchor == 0 {
        if snapshot.all_unread_loaded {
            snapshot.head.len().saturating_sub(1)
        } else {
            snapshot.head.len()
        }
    } else {
        0
    };
    let preferred_head = preferred_head_count(
        snapshot,
        call_encoding,
        head_available,
        budget.value.saturating_div(10).max(1),
    );
    let mut low = 0_usize;
    let mut high = preferred_head;
    let mut head_that_fits = None;
    while low <= high {
        let head = low + (high - low) / 2;
        let candidate = render_candidate(job_id, wait_ms, snapshot, call_encoding, head, 1);
        if estimate_tokens(&candidate.response) <= budget.value {
            head_that_fits = Some(head);
            low = head.saturating_add(1);
        } else if head == 0 {
            break;
        } else {
            high = head - 1;
        }
    }
    let head = head_that_fits.ok_or_else(|| budget_too_small_message(budget))?;
    let tail_limit = if snapshot.all_unread_loaded {
        tail_available.saturating_sub(head)
    } else {
        tail_available
    };
    let mut low = 1_usize;
    let mut high = tail_limit;
    let mut best = None;
    while low <= high {
        let tail = low + (high - low) / 2;
        let candidate = render_candidate(job_id, wait_ms, snapshot, call_encoding, head, tail);
        if estimate_tokens(&candidate.response) <= budget.value {
            best = Some(candidate);
            low = tail.saturating_add(1);
        } else if tail == 1 {
            break;
        } else {
            high = tail - 1;
        }
    }
    let candidate = best.ok_or_else(|| budget_too_small_message(budget))?;
    Ok(FormattedPage {
        response: candidate.response,
        cursor_seq: Some(snapshot.unread_last),
    })
}

fn preferred_head_count(
    snapshot: &OutputSnapshot,
    call_encoding: Option<OutputEncoding>,
    available: usize,
    token_target: usize,
) -> usize {
    let mut low = 0_usize;
    let mut high = available;
    let mut best = 0_usize;
    while low <= high {
        let count = low + (high - low) / 2;
        let selected = select_lines(snapshot, count, 0);
        let encoded = selected
            .iter()
            .map(|line| line.encoded_line())
            .collect::<Vec<_>>();
        let decoded = decode_job(&encoded, call_encoding, snapshot.default_encoding);
        if estimate_tokens(&decoded.lines.join("\n")) <= token_target {
            best = count;
            low = count.saturating_add(1);
        } else if count == 0 {
            break;
        } else {
            high = count - 1;
        }
    }
    best
}

fn render_candidate(
    job_id: &str,
    wait_ms: u64,
    snapshot: &OutputSnapshot,
    call_encoding: Option<OutputEncoding>,
    head_count: usize,
    tail_count: usize,
) -> RenderedCandidate {
    let selected = select_lines(snapshot, head_count, tail_count);
    let encoded = selected
        .iter()
        .map(|line| line.encoded_line())
        .collect::<Vec<_>>();
    let decoded = decode_job(&encoded, call_encoding, snapshot.default_encoding);
    let mut facts = Vec::new();
    if let Some(path) = snapshot.direct_log.as_ref() {
        for (first, last) in omitted_ranges(snapshot, &selected) {
            facts.push(omission_fact(first, last, path));
        }
    } else if snapshot.legacy_loss {
        facts.push(legacy_loss_fact(snapshot));
    }
    if let Some(error) = &snapshot.capture_error {
        facts.push(capture_failure_fact(error, snapshot.direct_log.as_deref()));
    }
    if let Some(truncation) = &snapshot.output_truncation {
        facts.push(output_truncation_fact(
            truncation,
            snapshot.direct_log.as_deref(),
        ));
    }
    if let Some(note) = job_garble_note(decoded.invalid_sequences, snapshot.anchor) {
        facts.push(note);
    }
    facts.extend(decoded.transcoding_note);
    if let Some(path) = snapshot.direct_log.as_ref() {
        for (line, truncated) in selected.iter().zip(&decoded.truncated_per_line) {
            if *truncated {
                facts.push(format!(
                    "line {} truncated at 2000 chars in this response; complete log at {}",
                    line.seq,
                    display_path(path)
                ));
            }
        }
    }
    let last_seq = selected.last().map(|line| line.seq);
    let head = output_head(job_id, wait_ms, snapshot, &selected, last_seq, &facts);
    RenderedCandidate {
        response: head.render_with_body(&decoded.lines.join("\n")),
        last_seq,
    }
}

fn select_lines(
    snapshot: &OutputSnapshot,
    head_count: usize,
    tail_count: usize,
) -> Vec<&StoredLine> {
    let mut selected = Vec::new();
    if snapshot.all_unread_loaded {
        let head = head_count.min(snapshot.head.len());
        selected.extend(snapshot.head.iter().take(head));
        let tail = tail_count.min(snapshot.head.len().saturating_sub(head));
        if tail > 0 {
            selected.extend(snapshot.head[snapshot.head.len() - tail..].iter());
        }
        return selected;
    }
    selected.extend(snapshot.head.iter().take(head_count));
    let tail = tail_count.min(snapshot.tail.len());
    if tail > 0 {
        let last_head = selected.last().map(|line| line.seq).unwrap_or(0);
        selected.extend(
            snapshot.tail[snapshot.tail.len() - tail..]
                .iter()
                .filter(|line| line.seq > last_head),
        );
    }
    selected
}

fn omitted_ranges(snapshot: &OutputSnapshot, selected: &[&StoredLine]) -> Vec<(u64, u64)> {
    if snapshot.unread_first > snapshot.unread_last {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut next = snapshot.unread_first;
    for line in selected {
        if line.seq > next {
            ranges.push((next, line.seq - 1));
        }
        next = line.seq.saturating_add(1);
    }
    if next <= snapshot.unread_last {
        ranges.push((next, snapshot.unread_last));
    }
    ranges
}

fn omission_fact(first: u64, last: u64, path: &Path) -> String {
    if first == last {
        format!(
            "line {first} omitted from the body; complete log at {}",
            display_path(path)
        )
    } else {
        format!(
            "lines {first}-{last} omitted from the body; complete log at {}",
            display_path(path)
        )
    }
}

fn legacy_loss_fact(snapshot: &OutputSnapshot) -> String {
    let expected = snapshot.anchor.saturating_add(1);
    let missing = snapshot.unread_first.saturating_sub(expected);
    if missing > 0 {
        format!(
            "{missing} earlier {} dropped from this legacy job record and cannot be retrieved",
            if missing == 1 {
                "line was"
            } else {
                "lines were"
            }
        )
    } else {
        "this legacy job record lost or truncated output that cannot be retrieved".to_string()
    }
}

fn capture_failure_fact(error: &model::CaptureErrorRecord, direct_log: Option<&Path>) -> String {
    match direct_log {
        Some(path) => format!(
            "output capture failed after stored line {}: {}; the process was not killed, and the log at {} stops there",
            error.after_seq,
            error.reason,
            display_path(path)
        ),
        None => format!(
            "output capture failed after stored line {}: {}; the process was not killed, and this legacy record stops there",
            error.after_seq, error.reason
        ),
    }
}

fn output_truncation_fact(
    truncation: &model::OutputTruncationRecord,
    direct_log: Option<&Path>,
) -> String {
    match direct_log {
        Some(path) => format!(
            "the job reached its {}-byte log limit after stored line {}; later output was drained but not persisted; preserved prefix at {}",
            truncation.limit_bytes,
            truncation.after_seq,
            display_path(path)
        ),
        None => format!(
            "the job reached its {}-byte output limit after stored line {}; later output was drained but not persisted",
            truncation.limit_bytes, truncation.after_seq
        ),
    }
}

fn output_head(
    job_id: &str,
    wait_ms: u64,
    snapshot: &OutputSnapshot,
    selected: &[&StoredLine],
    last_seq: Option<u64>,
    facts: &[String],
) -> HeadNote {
    let state = match &snapshot.status {
        JobStatus::Running => "running".to_string(),
        JobStatus::Exited(exit) if exit.was_killed() => "killed".to_string(),
        JobStatus::Exited(exit) => format!("exited {}", exit.exit_code),
        JobStatus::Interrupted => "interrupted".to_string(),
    };
    let ranges = selected_ranges(selected);
    let metric = if ranges.is_empty() {
        HeadMetric::count(0, "new line", "new lines")
    } else {
        HeadMetric::Coverage {
            unit: "lines",
            ranges,
            total: CoverageTotal::Exact(
                usize::try_from(snapshot.total_lines).unwrap_or(usize::MAX),
            ),
        }
    };
    let mut head = HeadNote::new(format!("job {job_id} {state}"), metric);
    if matches!(snapshot.status, JobStatus::Running) && selected.is_empty() {
        head = head.fact(format!("no new output within {wait_ms} ms"));
    }
    if let Some(path) = snapshot.direct_log.as_ref() {
        head = head.fact(format!("log at {}", display_path(path)));
    }
    let next = last_seq.unwrap_or(snapshot.anchor);
    let more = next < snapshot.unread_last
        && (!snapshot.all_unread_loaded
            || snapshot.head.last().is_some_and(|line| line.seq > next));
    if more {
        head = head.fact("older legacy output remains");
    }
    if matches!(snapshot.status, JobStatus::Interrupted) {
        head = head.fact("process ended without an exit record");
    }
    for fact in facts {
        head = head.fact(fact);
    }
    head
}

fn selected_ranges(selected: &[&StoredLine]) -> Vec<CoveredRange> {
    let mut raw = Vec::<(u64, u64)>::new();
    for line in selected {
        match raw.last_mut() {
            Some((_, last)) if line.seq == last.saturating_add(1) => *last = line.seq,
            _ => raw.push((line.seq, line.seq)),
        }
    }
    raw.into_iter()
        .map(|(first, last)| {
            CoveredRange::new(
                usize::try_from(first).unwrap_or(usize::MAX),
                usize::try_from(last).unwrap_or(usize::MAX),
            )
        })
        .collect()
}

fn format_job_list(
    records: Vec<JobRecord>,
    status: JobListStatus,
    offset: u64,
    limit: u64,
) -> ToolResponse {
    let mut budget = match global_token_budget() {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    loop {
        let response = format_job_list_with_budget(records.clone(), status, offset, limit, budget);
        let starved = response.is_error
            && response.content.iter().any(|content| {
                matches!(content, crate::ToolContent::Text(text) if text.to_ascii_lowercase().contains("too small to return"))
            });
        if starved {
            match relax_tool_token_budget(GLOBAL_TOKEN_BUDGET_ENV) {
                Ok(Some(expanded)) => {
                    budget = expanded;
                    continue;
                }
                Ok(None) => {}
                Err(error) => return ToolResponse::error(error),
            }
        }
        return response;
    }
}

fn format_job_list_with_budget(
    mut records: Vec<JobRecord>,
    status: JobListStatus,
    offset: u64,
    limit: u64,
    budget: TokenBudget,
) -> ToolResponse {
    records.retain(|record| match status {
        JobListStatus::Running => record.status.is_running(),
        JobListStatus::Finished => !record.status.is_running(),
        JobListStatus::All => true,
    });
    if records.is_empty() {
        return head_with_budget(
            HeadNote::new("jobs", HeadMetric::count(0, "job", "jobs")).render(),
            budget,
        );
    }
    records.sort_by(
        |left, right| match (left.status.is_running(), right.status.is_running()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => store::started_sort_key(right).cmp(&store::started_sort_key(left)),
            (false, false) => right
                .ended_sort_key
                .cmp(&left.ended_sort_key)
                .then_with(|| right.id.cmp(&left.id)),
        },
    );
    let running = records
        .iter()
        .filter(|record| record.status.is_running())
        .count() as u64;
    let finished = records.len() as u64 - running;
    let total = records.len();
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
    if start == total {
        return head_with_budget(
            HeadNote::new("jobs", HeadMetric::count(0, "job", "jobs"))
                .fact(format!("{total} jobs exist"))
                .render(),
            budget,
        );
    }
    let page_end = start
        .saturating_add(usize::try_from(limit).unwrap_or(usize::MAX))
        .min(total);
    let entries = records[start..page_end]
        .iter()
        .map(format_job_entry)
        .collect::<Vec<_>>();
    let complete = render_job_list(start, &entries, total, running, finished);
    if estimate_tokens(&complete) <= budget.value {
        return ToolResponse::text(complete);
    }
    let mut low = 1_usize;
    let mut high = entries.len();
    let mut best = None;
    while low <= high {
        let shown = low + (high - low) / 2;
        let response = render_job_list(start, &entries[..shown], total, running, finished);
        if estimate_tokens(&response) <= budget.value {
            best = Some(response);
            low = shown.saturating_add(1);
        } else if shown == 1 {
            break;
        } else {
            high = shown - 1;
        }
    }
    best.map_or_else(
        || ToolResponse::error(budget_too_small_message(budget)),
        ToolResponse::text,
    )
}

fn render_job_list(
    start: usize,
    entries: &[String],
    total: usize,
    running: u64,
    finished: u64,
) -> String {
    let first = start.saturating_add(1);
    let last = start.saturating_add(entries.len());
    HeadNote::new(
        "jobs",
        HeadMetric::Coverage {
            unit: "jobs",
            ranges: vec![CoveredRange::new(first, last)],
            total: CoverageTotal::Exact(total),
        },
    )
    .fact(format!("{running} running, {finished} finished"))
    .render_with_body(&entries.join("\n\n"))
}

fn format_job_entry(record: &JobRecord) -> String {
    let status = match &record.status {
        JobStatus::Running => "running".to_string(),
        JobStatus::Exited(exit) if exit.was_killed() => "killed".to_string(),
        JobStatus::Exited(exit) => format!("exited {}", exit.exit_code),
        JobStatus::Interrupted => "interrupted".to_string(),
    };
    format!(
        "{}  {status}; started {}\n  {} — {}",
        record.id,
        record.meta.started_at,
        single_line(&record.meta.cwd),
        truncate_command(&record.meta.command)
    )
}

fn truncate_command(command: &str) -> String {
    let command = single_line(command);
    let mut characters = command.chars();
    let prefix = characters.by_ref().take(120).collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

fn missing_job(job_id: &str) -> ToolResponse {
    ToolResponse::error(missing_job_text(job_id))
}

fn missing_job_text(job_id: &str) -> String {
    format!(
        "No such job: \"{job_id}\". It may never have existed, or its finished record was evicted by the job storage limit. List known jobs with job_list."
    )
}

fn job_event(job_id: &str, event: &str) -> String {
    HeadNote::new(format!("job {job_id}"), HeadMetric::event(event)).render()
}

fn head_with_budget(head: String, budget: TokenBudget) -> ToolResponse {
    if estimate_tokens(&head) <= budget.value {
        ToolResponse::text(head)
    } else {
        ToolResponse::error(budget_too_small_message(budget))
    }
}

fn global_head(head: String) -> ToolResponse {
    match global_token_budget() {
        Ok(budget) => head_with_budget(head, budget),
        Err(error) => ToolResponse::error(error),
    }
}

pub(crate) fn summaries(paths: &ControlPaths) -> Result<Vec<JobSummary>, JobRegistryError> {
    let mut records = store::scan_registry(&paths.jobs_dir)?.records;
    records.sort_by(
        |left, right| match (left.status.is_running(), right.status.is_running()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => store::started_sort_key(right).cmp(&store::started_sort_key(left)),
            (false, false) => right
                .ended_sort_key
                .cmp(&left.ended_sort_key)
                .then_with(|| right.id.cmp(&left.id)),
        },
    );
    Ok(records
        .into_iter()
        .map(|record| {
            let source_key = format!(
                "{}:{}:{}",
                record.meta.origin.server_pid,
                record
                    .meta
                    .origin
                    .server_started
                    .as_deref()
                    .unwrap_or("legacy"),
                record.meta.origin.server_cwd
            );
            let source = JobSourceSummary {
                tag: source_tag(&source_key),
                key: source_key,
                server_pid: record.meta.origin.server_pid,
                parent_executable: record.meta.origin.parent_executable,
                server_cwd: record.meta.origin.server_cwd,
            };
            JobSummary {
                id: record.id,
                command: record.meta.command,
                cwd: record.meta.cwd,
                started_at: record.meta.started_at,
                status: match record.status {
                    JobStatus::Running => JobSummaryStatus::Running,
                    JobStatus::Exited(exit) => JobSummaryStatus::Exited(exit.exit_code),
                    JobStatus::Interrupted => JobSummaryStatus::Interrupted,
                },
                source,
            }
        })
        .collect())
}

/// Runs one admission-serialized history maintenance pass without starting a new job.
pub(crate) fn reap_history(paths: &ControlPaths) -> Result<u64, String> {
    let _admission = admission::AdmissionGuard::acquire(paths)?;
    let limits = store::effective_limits(paths)?;
    store::reap(paths, limits.storage_limit_mib)
}

fn source_tag(source_key: &str) -> String {
    let hash = source_key
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("{:06x}", hash & 0x00ff_ffff)
}

pub(crate) fn running_summaries(paths: &ControlPaths) -> Result<Vec<JobSummary>, String> {
    Ok(summaries(paths)?
        .into_iter()
        .filter(|job| job.status == JobSummaryStatus::Running)
        .collect())
}

pub(crate) fn refresh_tail(
    paths: &ControlPaths,
    job_id: &str,
    max_lines: usize,
    tail: &mut JobTail,
) -> Result<usize, String> {
    let record =
        store::find_record(&paths.jobs_dir, job_id)?.ok_or_else(|| missing_job_text(job_id))?;
    let delta = store::read_log_delta(&record, &mut tail.cursor, max_lines)?;
    let appended = usize::try_from(delta.observed_lines).unwrap_or(usize::MAX);
    let default_encoding = record
        .meta
        .encoding
        .as_deref()
        .map(validate_output_encoding)
        .transpose()
        .map_err(|error| {
            format!("Cannot read job {job_id}: its stored output encoding is invalid ({error})")
        })?;
    let encoded = delta
        .lines
        .iter()
        .map(StoredLine::encoded_line)
        .collect::<Vec<_>>();
    tail.lines
        .extend(decode_job(&encoded, None, default_encoding).lines);
    if tail.lines.len() > max_lines {
        tail.lines.drain(..tail.lines.len() - max_lines);
    }
    tail.capture_error = delta.capture_error.map(|error| {
        format!(
            "Output capture failed after stored line {}: {}",
            error.after_seq, error.reason
        )
    });
    tail.output_truncation = delta.output_truncation.map(|truncation| {
        format!(
            "Output storage reached its {}-byte hard limit after stored line {}; later output was drained but not persisted.",
            truncation.limit_bytes, truncation.after_seq
        )
    });
    Ok(appended)
}

pub(crate) fn reap(paths: &ControlPaths) -> Result<u64, String> {
    let _admission = admission::AdmissionGuard::acquire(paths)?;
    let limits = store::effective_limits(paths)?;
    store::reap(paths, limits.storage_limit_mib)
}

pub(crate) fn acquire_unapply_admission(
    paths: &ControlPaths,
) -> Result<admission::AdmissionGuard, String> {
    admission::AdmissionGuard::acquire(paths)
}

pub(crate) fn kill_all_running(paths: &ControlPaths) -> Result<u64, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut killed = std::collections::BTreeSet::new();
    loop {
        let registry = store::scan_registry(&paths.jobs_dir)?;
        let pending = registry.pending_reservations;
        let running = registry
            .records
            .into_iter()
            .filter(|record| record.status.is_running())
            .map(|record| record.id)
            .collect::<Vec<_>>();
        if running.is_empty() && pending == 0 {
            return Ok(killed.len() as u64);
        }
        if Instant::now() >= deadline {
            return Err(
                "Cannot finish Unapply because background jobs are still starting or reappearing. Stop the agents starting jobs, wait for any startup to settle, then retry Unapply."
                    .to_string(),
            );
        }
        for id in running {
            terminate(paths, &id)?;
            killed.insert(id);
        }
        if pending > 0 {
            std::thread::sleep(REGISTRY_POLL);
        }
    }
}

pub(crate) fn kill_for_control(paths: &ControlPaths, job_id: &str) -> Result<String, String> {
    Ok(match terminate(paths, job_id)? {
        KillState::Killed => format!("Job {job_id} killed."),
        KillState::AlreadyExited(code) => {
            format!("Job {job_id} had already exited with code {code}.")
        }
        KillState::AlreadyInterrupted => format!("Job {job_id} had already been interrupted."),
    })
}

#[cfg(unix)]
pub(crate) fn run_bootstrap_entry() -> Result<(), String> {
    match host::run_bootstrap() {
        Ok(()) => Ok(()),
        Err(error) => {
            host::write_startup_error(&error);
            Err(error)
        }
    }
}

pub(crate) fn run_host_entry() -> Result<(), String> {
    host::run_job_host()
}

#[cfg(unix)]
pub(crate) fn run_watchdog_entry(pid: u32, started: String) -> Result<(), String> {
    host::run_watchdog(pid, started)
}

#[cfg(test)]
mod tests {
    use super::{BackgroundLaunch, JobManager, OutputSnapshot, format_snapshot};
    use crate::budget::TokenBudget;
    use crate::control::paths::ControlPaths;
    use crate::model::ToolContent;

    use crate::shell::jobs::model::{
        CaptureErrorRecord, ExitRecord, JobStatus, StoredLine, TerminationKind,
    };
    use std::path::PathBuf;

    fn exited(code: i32, ended_order: u64) -> JobStatus {
        JobStatus::Exited(ExitRecord {
            exit_code: code,
            total_lines: 0,
            had_loss: false,
            ended_at: "2026-07-16T10:00:09Z".to_string(),
            ended_at_unix_nanos: ended_order,
            termination: TerminationKind::Exited,
            capture_error: None,
            output_truncation: None,
        })
    }

    #[test]
    fn manager_from_before_unapply_cannot_start_another_job() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let generation = super::admission::observe_generation(&paths).unwrap();
        let manager = JobManager {
            paths: Ok(paths.clone()),
            executable: Ok(temp.path().join("fastctx")),
            admission_generation: Ok(generation),
            cursors: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            background: super::background::BackgroundTracker::default(),
        };
        let mut admission = super::admission::AdmissionGuard::acquire(&paths).unwrap();
        admission.advance_generation().unwrap();
        drop(admission);

        let bash = temp.path().join("unused-bash");
        let environment =
            crate::session::SessionEnvironment::new(temp.path().to_path_buf(), Vec::new());
        let response = manager.start(BackgroundLaunch {
            bash: &bash,
            command: "printf should-not-run",
            cwd: temp.path(),
            login_shell: false,
            encoding: None,
            environment: &environment,
            utf8_locale: "C.UTF-8",
        });
        assert!(response.is_error);
        match response.content.into_iter().next().unwrap() {
            ToolContent::Text(text) => assert_eq!(
                text,
                "This FastCtx server predates the most recent Unapply. Start a new ChatGPT/Codex session and retry run_background."
            ),
            ToolContent::Image { .. } => panic!("job errors return text"),
        }
        assert!(!paths.jobs_dir.exists());
    }

    #[test]
    fn direct_and_legacy_terminals_keep_their_capability_promises_separate() {
        let budget = TokenBudget {
            value: 8_500,
            variable: "FASTCTX_TOKEN_BUDGET",
        };
        let interrupted = format_snapshot(
            "j-000001",
            0,
            &OutputSnapshot {
                status: JobStatus::Interrupted,
                head: Vec::new(),
                tail: Vec::new(),
                unread_first: 4,
                unread_last: 3,
                all_unread_loaded: true,
                total_lines: 3,
                legacy_loss: false,
                capture_error: None,
                output_truncation: None,
                default_encoding: None,
                anchor: 3,
                direct_log: Some(PathBuf::from("/jobs/j-000001/output.log")),
            },
            None,
            budget,
        )
        .unwrap();
        assert!(interrupted.response.contains("log at"));
        assert!(interrupted.response.contains("output.log"));

        let capture = format_snapshot(
            "j-000002",
            0,
            &OutputSnapshot {
                status: exited(17, 1),
                head: vec![StoredLine {
                    seq: 1,
                    bytes: b"kept".to_vec(),
                    total_bytes: 4,
                    stream_encoding: None,
                    legacy_text: None,
                    known_truncated: false,
                }],
                tail: Vec::new(),
                unread_first: 1,
                unread_last: 1,
                all_unread_loaded: true,
                total_lines: 2,
                legacy_loss: true,
                capture_error: Some(CaptureErrorRecord {
                    after_seq: 1,
                    reason: "disk unavailable".to_string(),
                }),
                output_truncation: None,
                default_encoding: None,
                anchor: 0,
                direct_log: None,
            },
            None,
            budget,
        )
        .unwrap();
        assert!(capture.response.contains("this legacy record stops there"));
        assert!(capture.response.contains("cannot be retrieved"));
        assert!(!capture.response.contains("complete log at"));
        assert!(!capture.response.contains("offset="));
    }

    #[test]
    fn a_capture_failure_on_a_direct_log_keeps_the_exit_status_and_points_at_the_log() {
        let budget = TokenBudget {
            value: 8_500,
            variable: "FASTCTX_TOKEN_BUDGET",
        };
        let rendered = format_snapshot(
            "j-000003",
            0,
            &OutputSnapshot {
                status: exited(17, 1),
                head: vec![StoredLine {
                    seq: 1,
                    bytes: b"output".to_vec(),
                    total_bytes: 6,
                    stream_encoding: None,
                    legacy_text: None,
                    known_truncated: false,
                }],
                tail: Vec::new(),
                unread_first: 1,
                unread_last: 1,
                all_unread_loaded: true,
                total_lines: 1,
                legacy_loss: false,
                capture_error: Some(CaptureErrorRecord {
                    after_seq: 1,
                    reason: "disk unavailable".to_string(),
                }),
                output_truncation: None,
                default_encoding: None,
                anchor: 0,
                direct_log: Some(PathBuf::from("/jobs/j-000003/output.log")),
            },
            None,
            budget,
        )
        .unwrap();
        assert!(
            rendered
                .response
                .contains("output capture failed after stored line 1: disk unavailable"),
            "{}",
            rendered.response
        );
        assert!(
            rendered.response.contains("the process was not killed"),
            "{}",
            rendered.response
        );
        assert!(
            rendered.response.contains("output.log") && rendered.response.contains("stops there"),
            "{}",
            rendered.response
        );
        assert!(
            rendered.response.contains("exited 17"),
            "{}",
            rendered.response
        );
        assert!(!rendered.response.contains("legacy record"));
    }
}
