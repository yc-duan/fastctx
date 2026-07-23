//! Persistent background jobs whose supervisors and records outlive every MCP server session.

pub(crate) mod admission;
mod host;
mod identity;
mod model;
mod spool;
mod store;

use crate::budget::{TokenBudget, estimate_tokens};
use crate::control::paths::ControlPaths;
use crate::model::ToolResponse;
use crate::shell::encoding::{
    OutputEncoding, decode_job, job_garble_note, validate_output_encoding,
};
use crate::shell::output::{
    budget_too_small_message, compose_response, compose_response_with_tail, dropped_note,
    global_token_budget, job_output_token_budget, plural, terminal_response,
};
use crate::shell::{JobListStatus, JobOutputWaitFor};
use model::{JobRecord, JobStatus, LaunchSpec, SpoolLine, TerminationKind};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KILL_ACK_TIMEOUT: Duration = Duration::from_secs(6);
const REGISTRY_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub(crate) struct JobManager {
    paths: Result<ControlPaths, String>,
    executable: Result<PathBuf, String>,
    admission_generation: Result<u64, String>,
    cursors: Arc<Mutex<HashMap<String, u64>>>,
}

#[derive(Clone, Debug)]
struct OutputSnapshot {
    status: JobStatus,
    lines: Vec<SpoolLine>,
    dropped: u64,
    total_lines: u64,
    had_loss: bool,
    capture_error: Option<model::CaptureErrorRecord>,
    default_encoding: Option<OutputEncoding>,
    anchor: u64,
}

#[derive(Clone, Debug)]
struct FormattedPage {
    response: String,
    last_seq: Option<u64>,
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

/// Public three-state lifecycle used by CLI and TUI without exposing spool internals.
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
    cursor: TailCursor,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TailCursor {
    offsets: BTreeMap<PathBuf, u64>,
    last_seq: u64,
}

impl JobManager {
    pub(crate) fn new() -> Self {
        let paths = ControlPaths::discover();
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
        }
    }

    pub(crate) fn start(
        &self,
        bash: &Path,
        command: &str,
        cwd: &Path,
        login_shell: bool,
        encoding: Option<OutputEncoding>,
    ) -> ToolResponse {
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

        let budget = match global_token_budget() {
            Ok(budget) => budget,
            Err(error) => {
                store::remove_reserved_job(&job_dir);
                return ToolResponse::error(error);
            }
        };
        let terminal = format!("(Complete: job {job_id} started.)");
        if estimate_tokens(&terminal) > budget.value {
            store::remove_reserved_job(&job_dir);
            return ToolResponse::error(budget_too_small_message(budget));
        }
        let server_cwd = std::env::current_dir().unwrap_or_else(|_| cwd.to_path_buf());
        let spec = LaunchSpec {
            job_id: job_id.clone(),
            job_dir: job_dir.clone(),
            bash: bash.to_path_buf(),
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            login_shell,
            encoding: encoding.map(|encoding| encoding.label().to_string()),
            origin: store::origin_snapshot(&server_cwd),
        };
        match host::launch_supervisor(executable, &spec) {
            Ok(()) => ToolResponse::text(terminal),
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
        wait_for: JobOutputWaitFor,
        after_seq: Option<u64>,
        encoding: Option<OutputEncoding>,
        cancelled: impl Fn() -> bool,
    ) -> ToolResponse {
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let budget = match job_output_token_budget() {
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
        let (record, spool) = loop {
            if cancelled() {
                return ToolResponse::error(
                    "The job output wait was cancelled because the MCP request or server session ended."
                        .to_string(),
                );
            }
            let record = match store::find_record(&paths.jobs_dir, job_id) {
                Ok(Some(record)) => record,
                Ok(None) => return missing_job(job_id),
                Err(error) => return ToolResponse::error(error),
            };
            let spool = match store::read_spool(&record) {
                Ok(spool) => spool,
                Err(error) => return ToolResponse::error(error),
            };
            let has_output = spool.lines.iter().any(|line| line.seq > anchor);
            if wait_event_arrived(
                wait_for,
                has_output,
                record.status.is_running(),
                spool.capture_error.is_some(),
            ) || started.elapsed() >= wait
            {
                break (record, spool);
            }
            let remaining = wait.saturating_sub(started.elapsed());
            std::thread::sleep(remaining.min(REGISTRY_POLL));
        };
        let lines = spool
            .lines
            .into_iter()
            .filter(|line| line.seq > anchor)
            .collect::<Vec<_>>();
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
        let snapshot = OutputSnapshot {
            status: record.status,
            lines,
            dropped: spool.oldest_seq.saturating_sub(anchor.saturating_add(1)),
            total_lines: spool.total_lines,
            had_loss: spool.had_loss,
            capture_error: spool.capture_error,
            default_encoding,
            anchor,
        };
        let page = match format_snapshot(job_id, wait_ms, &snapshot, encoding, budget) {
            Ok(page) => page,
            Err(error) => return ToolResponse::error(error),
        };
        if let Some(last_seq) = page.last_seq {
            let mut cursors = self.cursors.lock().unwrap();
            let cursor = cursors.entry(job_id.to_string()).or_insert(0);
            *cursor = (*cursor).max(last_seq);
        }
        ToolResponse::text(page.response)
    }

    pub(crate) fn kill(&self, job_id: &str) -> ToolResponse {
        let paths = match self.paths() {
            Ok(paths) => paths,
            Err(error) => return ToolResponse::error(error),
        };
        let killed = format!("(Complete: job {job_id} killed.)");
        let budget = match global_token_budget() {
            Ok(budget) => budget,
            Err(error) => return ToolResponse::error(error),
        };
        if estimate_tokens(&killed) > budget.value {
            return ToolResponse::error(budget_too_small_message(budget));
        }
        match terminate(paths, job_id) {
            Ok(KillState::Killed) => ToolResponse::text(killed),
            Ok(KillState::AlreadyExited(code)) => global_terminal(format!(
                "(Complete: job {job_id} had already exited with code {code}.)"
            )),
            Ok(KillState::AlreadyInterrupted) => global_terminal(format!(
                "(Complete: job {job_id} had already been interrupted.)"
            )),
            Err(error) => ToolResponse::error(error),
        }
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
}

fn wait_event_arrived(
    wait_for: JobOutputWaitFor,
    has_output: bool,
    is_running: bool,
    capture_failed: bool,
) -> bool {
    !is_running || capture_failed || (wait_for == JobOutputWaitFor::Output && has_output)
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
            JobStatus::Exited(exit) if exit.termination == TerminationKind::Killed => {
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
    let mut fixed_notes = Vec::new();
    if let Some(note) = dropped_note(snapshot.dropped) {
        fixed_notes.push(note);
    }
    if let Some(error) = &snapshot.capture_error {
        fixed_notes.push(format!(
            "(Note: output capture failed after seq {}: {}. This does not kill the process; its exit status remains available, but redirect the command to a file for a full log.)",
            error.after_seq, error.reason
        ));
    }
    if snapshot.lines.is_empty() {
        let terminal = match &snapshot.status {
            JobStatus::Running => format!(
                "(Partial: job {job_id} is running; no new output within {wait_ms} ms. Call job_output again with a larger wait_ms (up to 240000) or wait_for=\"exit\", or do other work first and check back.)"
            ),
            status => complete_terminal(job_id, status, snapshot.total_lines, snapshot.had_loss),
        };
        let leading = (!fixed_notes.is_empty()).then(|| fixed_notes.join("\n\n"));
        let response = compose_response(leading.as_deref(), &[], &terminal);
        if estimate_tokens(&response) > budget.value {
            return Err(budget_too_small_message(budget));
        }
        return Ok(FormattedPage {
            response,
            last_seq: None,
        });
    }

    let mut low = 1;
    let mut high = snapshot.lines.len();
    let mut best = None;
    while low <= high {
        let shown = low + (high - low) / 2;
        let selected = &snapshot.lines[..shown];
        let last_seq = selected.last().expect("selected output is non-empty").seq;
        let encoded = selected
            .iter()
            .map(SpoolLine::encoded_line)
            .collect::<Vec<_>>();
        let decoded = decode_job(&encoded, call_encoding, snapshot.default_encoding);
        let terminal = page_terminal(
            job_id,
            &snapshot.status,
            shown,
            snapshot.lines.len(),
            snapshot.total_lines,
            snapshot.had_loss || decoded.had_truncation,
            last_seq,
        );
        let mut notes = fixed_notes.clone();
        if let Some(note) = job_garble_note(decoded.invalid_sequences, snapshot.anchor) {
            notes.push(note);
        }
        let leading = (!notes.is_empty()).then(|| notes.join("\n\n"));
        let response = compose_response_with_tail(
            leading.as_deref(),
            &decoded.lines,
            decoded.transcoding_note.as_deref(),
            &terminal,
        );
        if estimate_tokens(&response) <= budget.value {
            best = Some(FormattedPage {
                response,
                last_seq: Some(last_seq),
            });
            low = shown.saturating_add(1);
        } else if shown == 1 {
            break;
        } else {
            high = shown - 1;
        }
    }
    best.ok_or_else(|| budget_too_small_message(budget))
}

fn page_terminal(
    job_id: &str,
    status: &JobStatus,
    shown: usize,
    available: usize,
    total_lines: u64,
    had_loss: bool,
    next: u64,
) -> String {
    match status {
        JobStatus::Running => format!(
            "(Partial: job {job_id} is running; {shown} new {} shown. Call job_output again with after_seq={next} for more.)",
            plural(shown as u64, "line", "lines")
        ),
        JobStatus::Exited(exit) if shown < available => format!(
            "(Partial: job {job_id} exited {}; more output buffered. Call job_output again with after_seq={next}.)",
            exit.exit_code
        ),
        JobStatus::Interrupted if shown < available => format!(
            "(Partial: job {job_id} was interrupted; more output buffered. Call job_output again with after_seq={next}.)"
        ),
        status => complete_terminal(job_id, status, total_lines, had_loss),
    }
}

fn complete_terminal(job_id: &str, status: &JobStatus, total: u64, had_loss: bool) -> String {
    let loss = if had_loss {
        ", but some output was dropped or truncated — redirect the command to a file (command > file 2>&1) for the full log"
    } else {
        ""
    };
    match status {
        JobStatus::Running => unreachable!("running jobs never have a complete terminal"),
        JobStatus::Exited(exit) => format!(
            "(Complete: job {job_id} exited {}; {total} {} total{loss}.)",
            exit.exit_code,
            plural(total, "line", "lines")
        ),
        JobStatus::Interrupted => format!(
            "(Complete: job {job_id} was interrupted: its process ended without an exit record (machine restart or external kill); {total} {} preserved{loss}.)",
            plural(total, "line", "lines")
        ),
    }
}

fn format_job_list(
    records: Vec<JobRecord>,
    status: JobListStatus,
    offset: u64,
    limit: u64,
) -> ToolResponse {
    let budget = match global_token_budget() {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    format_job_list_with_budget(records, status, offset, limit, budget)
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
        return terminal_response(empty_job_list_terminal(status), budget);
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
        return terminal_response(
            format!(
                "(Complete: no {} at offset={offset}; {total} available.)",
                job_list_scope(status)
            ),
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
    let complete_terminal = complete_job_list_terminal(status, running, finished);
    let terminal = if page_end < total {
        partial_job_list_terminal(status, start, entries.len(), total, limit)
    } else {
        complete_terminal.clone()
    };
    let complete = compose_list(&entries, &terminal);
    if estimate_tokens(&complete) <= budget.value {
        return ToolResponse::text(complete);
    }
    let mut low = 1_usize;
    let mut high = entries.len();
    let mut best = None;
    while low <= high {
        let shown = low + (high - low) / 2;
        let terminal = partial_job_list_terminal(status, start, shown, total, limit);
        let response = compose_list(&entries[..shown], &terminal);
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

fn empty_job_list_terminal(status: JobListStatus) -> String {
    match status {
        JobListStatus::Running => "(Complete: no running jobs.)",
        JobListStatus::Finished => "(Complete: no finished records.)",
        JobListStatus::All => "(Complete: no jobs.)",
    }
    .to_string()
}

fn complete_job_list_terminal(status: JobListStatus, running: u64, finished: u64) -> String {
    match status {
        JobListStatus::Running => format!(
            "(Complete: {running} running {}.)",
            plural(running, "job", "jobs")
        ),
        JobListStatus::Finished => format!(
            "(Complete: {finished} finished {}.)",
            plural(finished, "record", "records")
        ),
        JobListStatus::All => format!(
            "(Complete: {running} running {}, {finished} finished {}.)",
            plural(running, "job", "jobs"),
            plural(finished, "record", "records")
        ),
    }
}

fn partial_job_list_terminal(
    status: JobListStatus,
    start: usize,
    shown: usize,
    total: usize,
    limit: u64,
) -> String {
    let first = start.saturating_add(1);
    let next = start.saturating_add(shown);
    format!(
        "(Partial: showing {first}-{next} of {total} {}. Call job_list again with status=\"{}\", limit={limit}, offset={next}.)",
        job_list_scope(status),
        job_list_status_name(status)
    )
}

fn job_list_scope(status: JobListStatus) -> &'static str {
    match status {
        JobListStatus::Running => "running jobs",
        JobListStatus::Finished => "finished records",
        JobListStatus::All => "jobs",
    }
}

fn job_list_status_name(status: JobListStatus) -> &'static str {
    match status {
        JobListStatus::Running => "running",
        JobListStatus::Finished => "finished",
        JobListStatus::All => "all",
    }
}

fn format_job_entry(record: &JobRecord) -> String {
    let status = match &record.status {
        JobStatus::Running => "running".to_string(),
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

fn compose_list(entries: &[String], terminal: &str) -> String {
    if entries.is_empty() {
        terminal.to_string()
    } else {
        format!("{}\n\n{terminal}", entries.join("\n\n"))
    }
}

fn missing_job(job_id: &str) -> ToolResponse {
    ToolResponse::error(missing_job_text(job_id))
}

fn missing_job_text(job_id: &str) -> String {
    format!(
        "No such job: \"{job_id}\". It may never have existed, or its finished record was evicted by the job storage limit. List known jobs with job_list."
    )
}

fn global_terminal(terminal: String) -> ToolResponse {
    match global_token_budget() {
        Ok(budget) => terminal_response(terminal, budget),
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
    let delta = store::read_spool_delta(&record, &mut tail.cursor)?;
    let appended = delta.lines.len();
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
        .map(SpoolLine::encoded_line)
        .collect::<Vec<_>>();
    tail.lines
        .extend(decode_job(&encoded, None, default_encoding).lines);
    if tail.lines.len() > max_lines {
        tail.lines.drain(..tail.lines.len() - max_lines);
    }
    tail.capture_error = delta.capture_error.map(|error| {
        format!(
            "Output capture failed after seq {}: {}",
            error.after_seq, error.reason
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
    use super::{
        JobManager, JobRegistryError, OutputSnapshot, format_job_list, format_job_list_with_budget,
        format_snapshot, source_tag, summaries, truncate_command, wait_event_arrived,
    };
    use crate::budget::TokenBudget;
    use crate::control::paths::ControlPaths;
    use crate::model::{ToolContent, ToolResponse};
    use crate::shell::jobs::model::{
        ExitRecord, JOB_SCHEMA_VERSION, JobMeta, JobRecord, JobStatus, META_FILE, OriginSnapshot,
        ProcessIdentity, TerminationKind,
    };
    use crate::shell::{JobListStatus, JobOutputWaitFor};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn response_text(response: ToolResponse) -> String {
        assert!(!response.is_error);
        match response.content.into_iter().next().unwrap() {
            ToolContent::Text(text) => text,
            ToolContent::Image { .. } => panic!("job tools return text"),
        }
    }

    fn record(
        id: &str,
        command: &str,
        cwd: &str,
        started_at: &str,
        started_order: u64,
        status: JobStatus,
        ended_order: u64,
    ) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            directory: PathBuf::from(format!("/jobs/{id}")),
            meta: JobMeta {
                schema_version: 1,
                command: command.to_string(),
                cwd: cwd.to_string(),
                login_shell: false,
                encoding: None,
                supervisor: ProcessIdentity {
                    pid: 42,
                    started: "token".to_string(),
                },
                origin: OriginSnapshot {
                    server_pid: 7,
                    server_started: Some("server-token".to_string()),
                    parent_pid: Some(6),
                    parent_executable: Some("codex".to_string()),
                    server_cwd: "/workspace".to_string(),
                },
                started_at: started_at.to_string(),
                started_at_unix_nanos: started_order,
                isolation_warning: None,
            },
            status,
            ended_sort_key: UNIX_EPOCH + Duration::from_nanos(ended_order),
        }
    }

    fn exited(code: i32, ended_order: u64) -> JobStatus {
        JobStatus::Exited(ExitRecord {
            exit_code: code,
            total_lines: 0,
            had_loss: false,
            ended_at: "2026-07-16T10:00:09Z".to_string(),
            ended_at_unix_nanos: ended_order,
            termination: TerminationKind::Exited,
            capture_error: None,
        })
    }

    #[test]
    fn source_tags_are_stable_and_long_enough_to_distinguish_many_sessions() {
        let tag = source_tag("123:process-start-token:C:/workspace");
        assert_eq!(tag.len(), 6);
        assert!(tag.chars().all(|character| character.is_ascii_hexdigit()));
        assert_eq!(tag, source_tag("123:process-start-token:C:/workspace"));
        assert_ne!(tag, source_tag("124:process-start-token:C:/workspace"));
    }

    #[test]
    fn registry_errors_preserve_permission_denial_without_parsing_os_text() {
        let denied = JobRegistryError::from_io(
            "Cannot list the background job registry".to_string(),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        let damaged = JobRegistryError::data("Damaged metadata".to_string());

        assert!(denied.is_permission_denied());
        assert!(!damaged.is_permission_denied());
    }

    #[test]
    fn registry_summary_aggregates_records_from_distinct_server_origins() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.jobs_dir).unwrap();
        let supervisor =
            super::identity::process_identity(std::process::id()).expect("test process is alive");

        for (id, server_pid, server_started, server_cwd) in [
            ("j-000001", 101, "server-a", "/workspace-a"),
            ("j-000002", 202, "server-b", "/workspace-b"),
        ] {
            let directory = paths.jobs_dir.join(id);
            std::fs::create_dir(&directory).unwrap();
            super::store::write_atomic_json(
                &directory.join(META_FILE),
                &JobMeta {
                    schema_version: JOB_SCHEMA_VERSION,
                    command: format!("printf {id}"),
                    cwd: server_cwd.to_string(),
                    login_shell: false,
                    encoding: None,
                    supervisor: supervisor.clone(),
                    origin: OriginSnapshot {
                        server_pid,
                        server_started: Some(server_started.to_string()),
                        parent_pid: None,
                        parent_executable: Some("codex".to_string()),
                        server_cwd: server_cwd.to_string(),
                    },
                    started_at: "2026-07-17T00:00:00Z".to_string(),
                    started_at_unix_nanos: u64::from(server_pid),
                    isolation_warning: None,
                },
            )
            .unwrap();
        }

        let records = summaries(&paths).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .map(|record| record.source.server_cwd.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            ["/workspace-a", "/workspace-b"].into_iter().collect()
        );
        assert_ne!(records[0].source.key, records[1].source.key);
        assert!(
            records
                .iter()
                .all(|record| record.status == super::JobSummaryStatus::Running)
        );
    }

    #[test]
    fn job_list_empty_and_sorted_complete_shapes_are_byte_exact() {
        assert_eq!(
            response_text(format_job_list(Vec::new(), JobListStatus::Running, 0, 20)),
            "(Complete: no running jobs.)"
        );
        let records = vec![
            record(
                "j-000003",
                "old finished",
                "/old-finished",
                "2026-07-16T10:00:00Z",
                1,
                exited(3, 3),
                3,
            ),
            record(
                "j-000001",
                "old running",
                "/old-running",
                "2026-07-16T10:00:00Z",
                1,
                JobStatus::Running,
                0,
            ),
            record(
                "j-000004",
                "new finished",
                "/new-finished",
                "2026-07-16T10:00:02Z",
                4,
                exited(4, 4),
                4,
            ),
            record(
                "j-000002",
                "new\ncommand",
                "/new-running",
                "2026-07-16T10:00:01Z",
                2,
                JobStatus::Running,
                0,
            ),
        ];
        assert_eq!(
            response_text(format_job_list(records, JobListStatus::All, 0, 20)),
            "j-000002  running; started 2026-07-16T10:00:01Z\n  /new-running — new\\ncommand\n\nj-000001  running; started 2026-07-16T10:00:00Z\n  /old-running — old running\n\nj-000004  exited 4; started 2026-07-16T10:00:02Z\n  /new-finished — new finished\n\nj-000003  exited 3; started 2026-07-16T10:00:00Z\n  /old-finished — old finished\n\n(Complete: 2 running jobs, 2 finished records.)"
        );
    }

    #[test]
    fn job_list_filters_before_offset_and_limit_and_preserves_the_query_in_continuations() {
        let records = vec![
            record(
                "j-000001",
                "old running",
                "/one",
                "2026-07-16T10:00:00Z",
                1,
                JobStatus::Running,
                0,
            ),
            record(
                "j-000002",
                "new running",
                "/two",
                "2026-07-16T10:00:01Z",
                2,
                JobStatus::Running,
                0,
            ),
            record(
                "j-000003",
                "old finished",
                "/three",
                "2026-07-16T10:00:02Z",
                3,
                exited(0, 3),
                3,
            ),
            record(
                "j-000004",
                "new finished",
                "/four",
                "2026-07-16T10:00:03Z",
                4,
                exited(7, 4),
                4,
            ),
        ];
        let budget = TokenBudget {
            value: 8_500,
            variable: "FASTCTX_TOKEN_BUDGET",
        };

        assert_eq!(
            response_text(format_job_list_with_budget(
                records.clone(),
                JobListStatus::Running,
                0,
                1,
                budget
            )),
            "j-000002  running; started 2026-07-16T10:00:01Z\n  /two — new running\n\n(Partial: showing 1-1 of 2 running jobs. Call job_list again with status=\"running\", limit=1, offset=1.)"
        );
        assert_eq!(
            response_text(format_job_list_with_budget(
                records.clone(),
                JobListStatus::Finished,
                1,
                20,
                budget
            )),
            "j-000003  exited 0; started 2026-07-16T10:00:02Z\n  /three — old finished\n\n(Complete: 2 finished records.)"
        );
        assert_eq!(
            response_text(format_job_list_with_budget(
                records,
                JobListStatus::Running,
                2,
                20,
                budget
            )),
            "(Complete: no running jobs at offset=2; 2 available.)"
        );
    }

    #[test]
    fn job_list_budget_pagination_and_offset_are_byte_exact() {
        let records = vec![
            record(
                "j-000001",
                "first",
                "/one",
                "2026-07-16T10:00:00Z",
                1,
                JobStatus::Running,
                0,
            ),
            record(
                "j-000002",
                "second",
                "/two",
                "2026-07-16T10:00:01Z",
                2,
                JobStatus::Running,
                0,
            ),
            record(
                "j-000003",
                "third",
                "/three",
                "2026-07-16T10:00:02Z",
                3,
                JobStatus::Running,
                0,
            ),
        ];
        let budget = TokenBudget {
            value: 80,
            variable: "FASTCTX_TOKEN_BUDGET",
        };
        assert_eq!(
            response_text(format_job_list_with_budget(
                records.clone(),
                JobListStatus::Running,
                0,
                20,
                budget
            )),
            "j-000003  running; started 2026-07-16T10:00:02Z\n  /three — third\n\n(Partial: showing 1-1 of 3 running jobs. Call job_list again with status=\"running\", limit=20, offset=1.)"
        );
        assert_eq!(
            response_text(format_job_list_with_budget(
                records,
                JobListStatus::Running,
                1,
                20,
                budget
            )),
            "j-000002  running; started 2026-07-16T10:00:01Z\n  /two — second\n\nj-000001  running; started 2026-07-16T10:00:00Z\n  /one — first\n\n(Complete: 3 running jobs.)"
        );
    }

    #[test]
    fn job_list_command_truncation_is_exactly_120_characters() {
        let short = "a".repeat(119);
        let exact = "b".repeat(120);
        let long = format!("{}c", "b".repeat(120));
        assert_eq!(truncate_command(&short), short);
        assert_eq!(truncate_command(&exact), exact);
        assert_eq!(truncate_command(&long), format!("{}…", "b".repeat(120)));
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
        };
        let mut admission = super::admission::AdmissionGuard::acquire(&paths).unwrap();
        admission.advance_generation().unwrap();
        drop(admission);

        let response = manager.start(
            &temp.path().join("unused-bash"),
            "printf should-not-run",
            temp.path(),
            false,
            None,
        );
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
    fn wait_modes_share_terminal_and_capture_failure_events_but_not_output_events() {
        for wait_for in [JobOutputWaitFor::Output, JobOutputWaitFor::Exit] {
            assert!(wait_event_arrived(wait_for, false, false, false));
            assert!(wait_event_arrived(wait_for, false, true, true));
            assert!(!wait_event_arrived(wait_for, false, true, false));
        }
        assert!(wait_event_arrived(
            JobOutputWaitFor::Output,
            true,
            true,
            false
        ));
        assert!(!wait_event_arrived(
            JobOutputWaitFor::Exit,
            true,
            true,
            false
        ));
    }

    #[test]
    fn interrupted_and_capture_loss_terminals_are_honest_and_final() {
        let budget = TokenBudget {
            value: 8_500,
            variable: "FASTCTX_TOKEN_BUDGET",
        };
        let interrupted = format_snapshot(
            "j-000001",
            0,
            &OutputSnapshot {
                status: JobStatus::Interrupted,
                lines: Vec::new(),
                dropped: 0,
                total_lines: 3,
                had_loss: false,
                capture_error: None,
                default_encoding: None,
                anchor: 0,
            },
            None,
            budget,
        )
        .unwrap();
        assert_eq!(
            interrupted.response,
            "(Complete: job j-000001 was interrupted: its process ended without an exit record (machine restart or external kill); 3 lines preserved.)"
        );

        let capture = format_snapshot(
            "j-000002",
            0,
            &OutputSnapshot {
                status: exited(17, 1),
                lines: Vec::new(),
                dropped: 0,
                total_lines: 2,
                had_loss: true,
                capture_error: Some(crate::shell::jobs::model::CaptureErrorRecord {
                    after_seq: 1,
                    reason: "disk unavailable".to_string(),
                }),
                default_encoding: None,
                anchor: 0,
            },
            None,
            budget,
        )
        .unwrap();
        assert_eq!(
            capture.response,
            "(Note: output capture failed after seq 1: disk unavailable. This does not kill the process; its exit status remains available, but redirect the command to a file for a full log.)\n\n(Complete: job j-000002 exited 17; 2 lines total, but some output was dropped or truncated — redirect the command to a file (command > file 2>&1) for the full log.)"
        );
    }
}
