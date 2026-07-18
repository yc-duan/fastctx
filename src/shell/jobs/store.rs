//! Directory-backed job registry, atomic records, spool reads, and terminal-record reaping.

use super::JobRegistryError;
use super::identity::{identity_is_alive, process_identity};
use super::model::{
    CAPTURE_ERROR_FILE, CaptureErrorRecord, EXIT_FILE, ExitRecord, JOB_SCHEMA_VERSION, JobMeta,
    JobRecord, JobStatus, KILL_REQUEST_FILE, META_FILE, OriginSnapshot, SpoolLine,
};
use crate::control::paths::ControlPaths;
use crate::control::settings::{DEFAULT_JOB_STORAGE_LIMIT_MIB, DEFAULT_MAX_RUNNING_JOBS};
use crate::edit::private_storage::ensure_private_directory;
use crate::paths::display_path;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeSet, hash_map::RandomState};
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PENDING_STALE_AFTER: Duration = Duration::from_secs(60);
const JOB_ID_SPACE: u64 = 36_u64.pow(6);
static JOB_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JobLimits {
    pub(crate) storage_limit_mib: u64,
    pub(crate) max_running_jobs: u64,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            storage_limit_mib: DEFAULT_JOB_STORAGE_LIMIT_MIB,
            max_running_jobs: DEFAULT_MAX_RUNNING_JOBS,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RegistrySnapshot {
    pub(crate) records: Vec<JobRecord>,
    pub(crate) pending_reservations: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SpoolSnapshot {
    pub(crate) lines: Vec<SpoolLine>,
    pub(crate) oldest_seq: u64,
    pub(crate) total_lines: u64,
    pub(crate) had_loss: bool,
    pub(crate) capture_error: Option<CaptureErrorRecord>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SpoolDelta {
    pub(super) lines: Vec<SpoolLine>,
    pub(super) capture_error: Option<CaptureErrorRecord>,
}

pub(crate) fn effective_limits(paths: &ControlPaths) -> Result<JobLimits, String> {
    let settings = crate::control::settings::load(paths)?;
    Ok(JobLimits {
        storage_limit_mib: positive_or_default(
            settings.fastshell.job_storage_limit_mib,
            DEFAULT_JOB_STORAGE_LIMIT_MIB,
        ),
        max_running_jobs: positive_or_default(
            settings.fastshell.max_running_jobs,
            DEFAULT_MAX_RUNNING_JOBS,
        ),
    })
}

fn positive_or_default(value: u64, fallback: u64) -> u64 {
    if value == 0 { fallback } else { value }
}

pub(crate) fn utc_now() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(crate) fn unix_nanos_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .unwrap_or(u64::MAX)
}

pub(crate) fn origin_snapshot(server_cwd: &Path) -> OriginSnapshot {
    let server_pid = std::process::id();
    let (parent_pid, parent_executable) = parent_process();
    OriginSnapshot {
        server_pid,
        server_started: process_identity(server_pid).map(|identity| identity.started),
        parent_pid,
        parent_executable,
        server_cwd: display_path(server_cwd),
    }
}

#[cfg(target_os = "linux")]
fn parent_process() -> (Option<u32>, Option<String>) {
    let parent = unsafe { libc::getppid() };
    let parent_pid = u32::try_from(parent).ok();
    let executable = std::fs::read_link(format!("/proc/{parent}/exe"))
        .ok()
        .map(|path| display_path(&path));
    (parent_pid, executable)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn parent_process() -> (Option<u32>, Option<String>) {
    let parent = unsafe { libc::getppid() };
    let output = std::process::Command::new("ps")
        .args(["-p", &parent.to_string(), "-o", "comm="])
        .output()
        .ok();
    let executable = output
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|executable| !executable.is_empty());
    (u32::try_from(parent).ok(), executable)
}

#[cfg(windows)]
fn parent_process() -> (Option<u32>, Option<String>) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the returned snapshot handle is closed on every path below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return (None, None);
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut rows = Vec::new();
    // SAFETY: entry has the documented size and remains writable for the enumeration.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) };
    while has_entry != 0 {
        let end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        rows.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..end]),
        ));
        // SAFETY: snapshot and entry remain valid for the next enumeration step.
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: this function owns the snapshot handle.
    unsafe {
        CloseHandle(snapshot);
    }
    let parent_pid = rows
        .iter()
        .find(|(pid, _, _)| *pid == std::process::id())
        .map(|(_, parent, _)| *parent);
    let executable = parent_pid.and_then(|parent| {
        rows.into_iter()
            .find(|(pid, _, _)| *pid == parent)
            .map(|(_, _, executable)| executable)
            .filter(|executable| !executable.is_empty())
    });
    (parent_pid, executable)
}

#[cfg(not(any(unix, windows)))]
fn parent_process() -> (Option<u32>, Option<String>) {
    (None, None)
}

pub(crate) fn ensure_root(root: &Path) -> Result<(), String> {
    ensure_private_directory(root, "background job registry")
}

pub(crate) fn reserve_job(root: &Path) -> Result<(String, PathBuf), String> {
    ensure_root(root)?;
    for _ in 0..256 {
        let id = generate_job_id();
        let directory = root.join(&id);
        match fs::create_dir(&directory) {
            Ok(()) => {
                ensure_private_directory(&directory, "background job")?;
                return Ok((id, directory));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot reserve a background job directory {}: {error}",
                    display_path(&directory)
                ));
            }
        }
    }
    Err(
        "Cannot allocate a unique background job id after 256 attempts. Retry the command."
            .to_string(),
    )
}

pub(crate) fn remove_reserved_job(directory: &Path) {
    match fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

pub(crate) fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Cannot encode {}: {error}", display_path(path)))?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Cannot write {} because it has no parent directory.",
            display_path(path)
        )
    })?;
    let mut temp = None;
    for attempt in 0..64_u64 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id(),
            attempt
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(&bytes).and_then(|()| file.write_all(b"\n")) {
                    drop(file);
                    let _ = fs::remove_file(&candidate);
                    return Err(format!(
                        "Cannot write temporary job record {}: {error}",
                        display_path(&candidate)
                    ));
                }
                drop(file);
                temp = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot create temporary job record {}: {error}",
                    display_path(&candidate)
                ));
            }
        }
    }
    let temp = temp.ok_or_else(|| {
        format!(
            "Cannot allocate a temporary file for job record {}.",
            display_path(path)
        )
    })?;
    publish_new(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "Cannot replace immutable job record {} because it already exists.",
                display_path(path)
            )
        } else {
            format!(
                "Cannot publish immutable job record {}: {error}",
                display_path(path)
            )
        }
    })?;
    Ok(())
}

#[cfg(unix)]
fn publish_new(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::hard_link(temporary, target)?;
    let _ = fs::remove_file(temporary);
    Ok(())
}

#[cfg(windows)]
fn publish_new(temporary: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let moved = unsafe {
        MoveFileExW(
            wide(temporary).as_ptr(),
            wide(target).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        let error = std::io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(80 | 183)) {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                error,
            ))
        } else {
            Err(error)
        }
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn publish_new(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::hard_link(temporary, target)?;
    let _ = fs::remove_file(temporary);
    Ok(())
}

pub(crate) fn read_json<T: DeserializeOwned>(
    path: &Path,
    label: &str,
) -> Result<Option<T>, JobRegistryError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(JobRegistryError::from_io(
                format!("Cannot read {label} {}", display_path(path)),
                error,
            ));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        JobRegistryError::data(format!(
            "Cannot parse {label} {}: {error}. Remove the damaged finished record or restart its running job.",
            display_path(path)
        ))
    })
}

pub(crate) fn scan_registry(root: &Path) -> Result<RegistrySnapshot, JobRegistryError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RegistrySnapshot::default());
        }
        Err(error) => {
            return Err(JobRegistryError::from_io(
                format!(
                    "Cannot list the background job registry {}",
                    display_path(root)
                ),
                error,
            ));
        }
    };
    let now = SystemTime::now();
    let mut snapshot = RegistrySnapshot::default();
    for entry in entries {
        let entry = entry.map_err(|error| {
            JobRegistryError::from_io(
                format!(
                    "Cannot inspect an entry in the background job registry {}",
                    display_path(root)
                ),
                error,
            )
        })?;
        let id = entry.file_name().to_string_lossy().to_string();
        if !valid_job_id(&id) {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(JobRegistryError::from_io(
                    format!(
                        "Cannot inspect background job {id} at {}",
                        display_path(&entry.path())
                    ),
                    error,
                ));
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(JobRegistryError::data(format!(
                "Cannot use background job path {} because it is not a private directory. Remove it and retry.",
                display_path(&entry.path())
            )));
        }
        let directory = entry.path();
        let Some(meta) = read_json::<JobMeta>(&directory.join(META_FILE), "job metadata")? else {
            let age = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or_default();
            if age >= PENDING_STALE_AFTER {
                remove_reserved_job(&directory);
            } else {
                snapshot.pending_reservations = snapshot.pending_reservations.saturating_add(1);
            }
            continue;
        };
        if meta.schema_version != JOB_SCHEMA_VERSION {
            return Err(JobRegistryError::data(format!(
                "Cannot read job {id}: metadata schema {} is unsupported by this FastCtx (expected {}). Upgrade FastCtx or remove the finished record.",
                meta.schema_version, JOB_SCHEMA_VERSION
            )));
        }
        let exit = read_json::<ExitRecord>(&directory.join(EXIT_FILE), "job exit record")?;
        let status = if let Some(exit) = exit {
            JobStatus::Exited(exit)
        } else if identity_is_alive(&meta.supervisor) {
            JobStatus::Running
        } else {
            JobStatus::Interrupted
        };
        let ended_sort_key = terminal_sort_key(&directory, &status);
        if !directory.exists() {
            continue;
        }
        snapshot.records.push(JobRecord {
            id,
            directory,
            meta,
            status,
            ended_sort_key,
        });
    }
    Ok(snapshot)
}

fn terminal_sort_key(directory: &Path, status: &JobStatus) -> SystemTime {
    if let JobStatus::Exited(exit) = status
        && exit.ended_at_unix_nanos > 0
    {
        return UNIX_EPOCH + Duration::from_nanos(exit.ended_at_unix_nanos);
    }
    let preferred = match status {
        JobStatus::Exited(_) => directory.join(EXIT_FILE),
        JobStatus::Interrupted => directory.to_path_buf(),
        JobStatus::Running => directory.join(META_FILE),
    };
    fs::metadata(preferred)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

pub(super) fn started_sort_key(record: &JobRecord) -> (u64, &str) {
    (record.meta.started_at_unix_nanos, record.id.as_str())
}

pub(crate) fn find_record(root: &Path, job_id: &str) -> Result<Option<JobRecord>, String> {
    if !valid_job_id(job_id) {
        return Ok(None);
    }
    Ok(scan_registry(root)?
        .records
        .into_iter()
        .find(|record| record.id == job_id))
}

pub(crate) fn read_spool(record: &JobRecord) -> Result<SpoolSnapshot, String> {
    let mut segments = segment_paths(&record.directory)?;
    segments.sort();
    let mut snapshot = SpoolSnapshot::default();
    for path in segments {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot read job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                ));
            }
        };
        let mut reader = BufReader::new(file);
        let mut source = Vec::new();
        loop {
            source.clear();
            let bytes = reader.read_until(b'\n', &mut source).map_err(|error| {
                format!(
                    "Cannot read job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                )
            })?;
            if bytes == 0 {
                break;
            }
            if !source.ends_with(b"\n") {
                if !record.status.is_running() {
                    snapshot.had_loss = true;
                }
                break;
            }
            let line = match serde_json::from_slice::<SpoolLine>(trim_record_ending(&source)) {
                Ok(line) => line,
                Err(error) => {
                    return Err(format!(
                        "Cannot parse job {} output segment {}: {error}. The record is damaged.",
                        record.id,
                        display_path(&path)
                    ));
                }
            };
            snapshot.total_lines = snapshot.total_lines.max(line.seq);
            snapshot.had_loss |= line.had_loss || line.truncated;
            snapshot.lines.push(line);
        }
    }
    snapshot.lines.sort_by_key(|line| line.seq);
    snapshot.lines.dedup_by_key(|line| line.seq);
    snapshot.oldest_seq = snapshot
        .lines
        .first()
        .map(|line| line.seq)
        .unwrap_or_else(|| match &record.status {
            JobStatus::Exited(exit) => exit.total_lines.saturating_add(1),
            _ => snapshot.total_lines.saturating_add(1),
        });
    if snapshot.oldest_seq > 1 {
        snapshot.had_loss = true;
    }
    if let JobStatus::Exited(exit) = &record.status {
        snapshot.total_lines = snapshot.total_lines.max(exit.total_lines);
        snapshot.had_loss |= exit.had_loss;
    }
    snapshot.capture_error = read_json(
        &record.directory.join(CAPTURE_ERROR_FILE),
        "job capture-error record",
    )?;
    if snapshot.capture_error.is_none()
        && let JobStatus::Exited(exit) = &record.status
    {
        snapshot.capture_error.clone_from(&exit.capture_error);
    }
    Ok(snapshot)
}

pub(super) fn read_spool_delta(
    record: &JobRecord,
    cursor: &mut super::TailCursor,
) -> Result<SpoolDelta, String> {
    let mut segments = segment_paths(&record.directory)?;
    segments.sort();
    let present = segments.iter().cloned().collect::<BTreeSet<_>>();
    let mut next = cursor.clone();
    next.offsets.retain(|path, _| present.contains(path));
    let mut delta = SpoolDelta::default();

    for path in segments {
        let length = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                next.offsets.remove(&path);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Cannot inspect job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                ));
            }
        };
        let mut offset = next.offsets.get(&path).copied().unwrap_or(0);
        if offset > length {
            offset = 0;
        }
        if offset == length {
            continue;
        }
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                next.offsets.remove(&path);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Cannot read job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                ));
            }
        };
        file.seek(SeekFrom::Start(offset)).map_err(|error| {
            format!(
                "Cannot seek job {} output segment {}: {error}",
                record.id,
                display_path(&path)
            )
        })?;
        let mut reader = BufReader::new(file);
        let mut source = Vec::new();
        let mut committed = offset;
        loop {
            source.clear();
            let record_start = reader.stream_position().map_err(|error| {
                format!(
                    "Cannot locate job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                )
            })?;
            let bytes = reader.read_until(b'\n', &mut source).map_err(|error| {
                format!(
                    "Cannot read job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                )
            })?;
            if bytes == 0 {
                break;
            }
            if !source.ends_with(b"\n") {
                committed = record_start;
                break;
            }
            let line = serde_json::from_slice::<SpoolLine>(trim_record_ending(&source)).map_err(
                |error| {
                    format!(
                        "Cannot parse job {} output segment {}: {error}. The record is damaged.",
                        record.id,
                        display_path(&path)
                    )
                },
            )?;
            if line.seq > cursor.last_seq {
                delta.lines.push(line.clone());
            }
            next.last_seq = next.last_seq.max(line.seq);
            committed = reader.stream_position().map_err(|error| {
                format!(
                    "Cannot locate job {} output segment {}: {error}",
                    record.id,
                    display_path(&path)
                )
            })?;
        }
        next.offsets.insert(path, committed);
    }

    delta.lines.sort_by_key(|line| line.seq);
    delta.lines.dedup_by_key(|line| line.seq);
    delta.capture_error = read_json(
        &record.directory.join(CAPTURE_ERROR_FILE),
        "job capture-error record",
    )?;
    if delta.capture_error.is_none()
        && let JobStatus::Exited(exit) = &record.status
    {
        delta.capture_error.clone_from(&exit.capture_error);
    }
    *cursor = next;
    Ok(delta)
}

fn trim_record_ending(record: &[u8]) -> &[u8] {
    let record = record.strip_suffix(b"\n").unwrap_or(record);
    record.strip_suffix(b"\r").unwrap_or(record)
}

pub(crate) fn segment_paths(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "Cannot list job output segments in {}: {error}",
            display_path(directory)
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect a job output segment in {}: {error}",
                display_path(directory)
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("segment-") && name.ends_with(".jsonl") {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

pub(crate) fn request_kill(record: &JobRecord) -> Result<(), String> {
    let path = record.directory.join(KILL_REQUEST_FILE);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!(
            "Cannot request termination of job {}: {error}. Retry job_kill.",
            record.id
        )),
    }
}

pub(crate) fn kill_requested(directory: &Path) -> bool {
    directory.join(KILL_REQUEST_FILE).is_file()
}

pub(crate) fn clear_kill_request(directory: &Path) {
    match fs::remove_file(directory.join(KILL_REQUEST_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

pub(crate) fn reap(paths: &ControlPaths, limit_mib: u64) -> Result<u64, String> {
    let root = &paths.jobs_dir;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "Cannot inspect job storage {}: {error}",
                display_path(root)
            ));
        }
    };
    let mut total = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("Cannot inspect job storage {}: {error}", display_path(root))
        })?;
        total = total.saturating_add(path_size(&entry.path())?);
    }
    let limit = limit_mib.saturating_mul(1024 * 1024);
    if total <= limit {
        return Ok(0);
    }
    let target = limit.saturating_mul(9) / 10;
    let mut terminal = scan_registry(root)?
        .records
        .into_iter()
        .filter(|record| !record.status.is_running())
        .collect::<Vec<_>>();
    terminal.sort_by_key(|record| record.ended_sort_key);
    let mut removed = 0_u64;
    for record in terminal {
        if total <= target {
            break;
        }
        let bytes = path_size(&record.directory).unwrap_or(0);
        match fs::remove_dir_all(&record.directory) {
            Ok(()) => {
                total = total.saturating_sub(bytes);
                removed = removed.saturating_add(1);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                total = total.saturating_sub(bytes);
            }
            Err(error) => {
                return Err(format!(
                    "Cannot evict finished job {} from {}: {error}",
                    record.id,
                    display_path(&record.directory)
                ));
            }
        }
    }
    Ok(removed)
}

fn path_size(path: &Path) -> Result<u64, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Cannot inspect job storage {}: {error}", display_path(path)))?;
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)
        .map_err(|error| format!("Cannot inspect job storage {}: {error}", display_path(path)))?
    {
        let entry = entry.map_err(|error| {
            format!("Cannot inspect job storage {}: {error}", display_path(path))
        })?;
        total = total.saturating_add(path_size(&entry.path())?);
    }
    Ok(total)
}

pub(crate) fn valid_job_id(id: &str) -> bool {
    id.len() == 8
        && id.starts_with("j-")
        && id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

fn generate_job_id() -> String {
    let sequence = JOB_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = RandomState::new().build_hasher();
    sequence.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    format!("j-{}", base36(hasher.finish() % JOB_ID_SPACE))
}

fn base36(mut value: u64) -> String {
    let mut bytes = [b'0'; 6];
    for index in (0..bytes.len()).rev() {
        let digit = (value % 36) as u8;
        bytes[index] = if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        };
        value /= 36;
    }
    String::from_utf8(bytes.to_vec()).expect("base36 output is ASCII")
}

#[cfg(test)]
mod tests {
    use super::{
        read_spool, read_spool_delta, reap, scan_registry, valid_job_id, write_atomic_json,
    };
    use crate::control::paths::ControlPaths;
    use crate::control::settings::{FastCtxSettings, save};
    use crate::shell::jobs::TailCursor;
    use crate::shell::jobs::identity::process_identity;
    use crate::shell::jobs::model::{
        CaptureErrorRecord, EXIT_FILE, ExitRecord, JOB_SCHEMA_VERSION, JobMeta, META_FILE,
        OriginSnapshot, ProcessIdentity, SpoolLine, TerminationKind,
    };
    use filetime::{FileTime, set_file_mtime};
    use std::path::{Path, PathBuf};

    fn metadata(supervisor: ProcessIdentity, started_at_unix_nanos: u64) -> JobMeta {
        JobMeta {
            schema_version: JOB_SCHEMA_VERSION,
            command: "printf test".to_string(),
            cwd: "/fixture".to_string(),
            login_shell: false,
            supervisor,
            origin: OriginSnapshot {
                server_pid: 7,
                server_started: Some("server-token".to_string()),
                parent_pid: Some(6),
                parent_executable: Some("codex".to_string()),
                server_cwd: "/fixture".to_string(),
            },
            started_at: "2026-07-16T10:00:00Z".to_string(),
            started_at_unix_nanos,
            isolation_warning: None,
        }
    }

    fn terminal_job(
        root: &Path,
        id: &str,
        ended_at_unix_nanos: u64,
        payload_bytes: usize,
    ) -> PathBuf {
        let directory = root.join(id);
        std::fs::create_dir_all(&directory).unwrap();
        let supervisor = process_identity(std::process::id()).unwrap();
        write_atomic_json(&directory.join(META_FILE), &metadata(supervisor, 1)).unwrap();
        let exit = ExitRecord {
            exit_code: 0,
            total_lines: 0,
            had_loss: false,
            ended_at: "2026-07-16T10:00:01Z".to_string(),
            ended_at_unix_nanos,
            termination: TerminationKind::Exited,
            capture_error: None,
        };
        write_atomic_json(&directory.join(EXIT_FILE), &exit).unwrap();
        std::fs::write(directory.join("payload.bin"), vec![b'x'; payload_bytes]).unwrap();
        directory
    }

    #[test]
    fn immutable_records_are_published_without_a_clobber_window() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("meta.json");
        write_atomic_json(&path, &serde_json::json!({"value": 1})).unwrap();
        let error = write_atomic_json(&path, &serde_json::json!({"value": 2})).unwrap_err();

        assert!(error.contains("immutable job record"), "{error}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap(),
            serde_json::json!({"value": 1})
        );
    }

    #[test]
    fn pid_creation_token_mismatch_is_interrupted_not_running() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let directory = root.join("j-000001");
        std::fs::create_dir_all(&directory).unwrap();
        write_atomic_json(
            &directory.join(META_FILE),
            &metadata(
                ProcessIdentity {
                    pid: std::process::id(),
                    started: "definitely-not-this-process".to_string(),
                },
                1,
            ),
        )
        .unwrap();

        let snapshot = scan_registry(&root).unwrap();
        assert_eq!(snapshot.records.len(), 1);
        assert!(matches!(
            snapshot.records[0].status,
            crate::shell::jobs::model::JobStatus::Interrupted
        ));
    }

    #[test]
    fn registry_counts_fresh_reservations_and_reaps_crash_stale_ones() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let fresh = root.join("j-000001");
        let stale = root.join("j-000002");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        set_file_mtime(&stale, FileTime::from_unix_time(1, 0)).unwrap();

        let snapshot = scan_registry(&root).unwrap();

        assert_eq!(snapshot.pending_reservations, 1);
        assert!(fresh.exists());
        assert!(!stale.exists());
    }

    #[test]
    fn reaper_uses_terminal_end_order_and_stops_at_the_ninety_percent_low_water() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.jobs_dir).unwrap();
        let oldest = terminal_job(&paths.jobs_dir, "j-000001", 1, 430 * 1024);
        let middle = terminal_job(&paths.jobs_dir, "j-000002", 2, 430 * 1024);
        let newest = terminal_job(&paths.jobs_dir, "j-000003", 3, 430 * 1024);

        assert_eq!(reap(&paths, 1).unwrap(), 1);
        assert!(!oldest.exists());
        assert!(middle.exists());
        assert!(newest.exists());
    }

    #[test]
    fn saving_a_smaller_current_user_limit_immediately_reaps_finished_records() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.jobs_dir).unwrap();
        let oldest = terminal_job(&paths.jobs_dir, "j-000001", 1, 430 * 1024);
        let middle = terminal_job(&paths.jobs_dir, "j-000002", 2, 430 * 1024);
        let newest = terminal_job(&paths.jobs_dir, "j-000003", 3, 430 * 1024);
        let running = paths.jobs_dir.join("j-000004");
        std::fs::create_dir_all(&running).unwrap();
        write_atomic_json(
            &running.join(META_FILE),
            &metadata(process_identity(std::process::id()).unwrap(), 4),
        )
        .unwrap();

        let mut settings = FastCtxSettings::default();
        settings.fastshell.job_storage_limit_mib = 1;
        assert!(save(&paths, &settings).unwrap());

        assert!(!oldest.exists());
        assert!(middle.exists());
        assert!(newest.exists());
        assert!(running.exists());
        assert_eq!(
            crate::control::settings::load(&paths)
                .unwrap()
                .fastshell
                .job_storage_limit_mib,
            1
        );
    }

    #[test]
    fn reaper_never_deletes_a_running_record_even_when_it_alone_exceeds_the_limit() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let running = paths.jobs_dir.join("j-000001");
        std::fs::create_dir_all(&running).unwrap();
        write_atomic_json(
            &running.join(META_FILE),
            &metadata(process_identity(std::process::id()).unwrap(), 1),
        )
        .unwrap();
        std::fs::write(running.join("payload.bin"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let finished = terminal_job(&paths.jobs_dir, "j-000002", 2, 512 * 1024);

        assert_eq!(reap(&paths, 1).unwrap(), 1);
        assert!(running.exists());
        assert!(!finished.exists());
        assert!(matches!(
            scan_registry(&paths.jobs_dir).unwrap().records[0].status,
            crate::shell::jobs::model::JobStatus::Running
        ));
    }

    #[test]
    fn partial_tail_records_are_ignored_and_exit_fallback_preserves_capture_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let directory = terminal_job(&root, "j-000001", 1, 0);
        std::fs::remove_file(directory.join(EXIT_FILE)).unwrap();
        let capture_error = CaptureErrorRecord {
            after_seq: 1,
            reason: "disk became unavailable".to_string(),
        };
        let exit = ExitRecord {
            exit_code: 17,
            total_lines: 2,
            had_loss: true,
            ended_at: "2026-07-16T10:00:01Z".to_string(),
            ended_at_unix_nanos: 1,
            termination: TerminationKind::Exited,
            capture_error: Some(capture_error.clone()),
        };
        write_atomic_json(&directory.join(EXIT_FILE), &exit).unwrap();
        let complete = serde_json::to_string(&SpoolLine {
            seq: 1,
            text: "kept".to_string(),
            truncated: false,
            had_loss: false,
        })
        .unwrap();
        let partial = serde_json::to_string(&SpoolLine {
            seq: 2,
            text: "partial".to_string(),
            truncated: false,
            had_loss: false,
        })
        .unwrap();
        std::fs::write(
            directory.join("segment-00000000000000000001.jsonl"),
            format!("{complete}\n{partial}"),
        )
        .unwrap();

        let record = scan_registry(&root).unwrap().records.remove(0);
        let spool = read_spool(&record).unwrap();
        assert_eq!(
            spool
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["kept"]
        );
        assert_eq!(spool.total_lines, 2);
        assert_eq!(spool.capture_error, Some(capture_error));
        assert!(spool.had_loss);
    }

    #[test]
    fn incomplete_terminal_spool_record_is_reported_as_output_loss() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let directory = terminal_job(&root, "j-000001", 1, 0);
        std::fs::remove_file(directory.join(EXIT_FILE)).unwrap();
        write_atomic_json(
            &directory.join(EXIT_FILE),
            &ExitRecord {
                exit_code: 0,
                total_lines: 2,
                had_loss: false,
                ended_at: "2026-07-16T10:00:01Z".to_string(),
                ended_at_unix_nanos: 1,
                termination: TerminationKind::Exited,
                capture_error: None,
            },
        )
        .unwrap();
        let complete = serde_json::to_string(&SpoolLine {
            seq: 1,
            text: "kept".to_string(),
            truncated: false,
            had_loss: false,
        })
        .unwrap();
        let mut segment = format!("{complete}\n{{\"seq\":2,\"text\":\"cut-off-").into_bytes();
        segment.extend_from_slice(&"界".as_bytes()[..2]);
        std::fs::write(
            directory.join("segment-00000000000000000001.jsonl"),
            segment,
        )
        .unwrap();

        let record = scan_registry(&root).unwrap().records.remove(0);
        let spool = read_spool(&record).unwrap();

        assert_eq!(
            spool
                .lines
                .iter()
                .map(|line| (line.seq, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "kept")]
        );
        assert_eq!(spool.total_lines, 2);
        assert!(spool.had_loss);
    }

    #[test]
    fn incremental_tail_reads_only_complete_records_appended_after_its_cursor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let directory = terminal_job(&root, "j-000001", 1, 0);
        let segment = directory.join("segment-00000000000000000001.jsonl");
        let encoded = |seq: u64, text: &str| {
            serde_json::to_string(&SpoolLine {
                seq,
                text: text.to_string(),
                truncated: false,
                had_loss: false,
            })
            .unwrap()
        };
        std::fs::write(
            &segment,
            format!("{}\n{}", encoded(1, "first"), encoded(2, "partial")),
        )
        .unwrap();
        let record = scan_registry(&root).unwrap().records.remove(0);
        let mut cursor = TailCursor::default();

        let first = read_spool_delta(&record, &mut cursor).unwrap();
        assert_eq!(
            first
                .lines
                .iter()
                .map(|line| (line.seq, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "first")]
        );
        assert_eq!(cursor.last_seq, 1);
        assert!(
            read_spool_delta(&record, &mut cursor)
                .unwrap()
                .lines
                .is_empty()
        );

        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&segment)
            .unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        let completed = read_spool_delta(&record, &mut cursor).unwrap();
        assert_eq!(
            completed
                .lines
                .iter()
                .map(|line| (line.seq, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(2, "partial")]
        );
        assert_eq!(cursor.last_seq, 2);
        assert!(
            read_spool_delta(&record, &mut cursor)
                .unwrap()
                .lines
                .is_empty()
        );

        std::fs::remove_file(&segment).unwrap();
        let next_segment = directory.join("segment-00000000000000000003.jsonl");
        std::fs::write(&next_segment, format!("{}\n", encoded(3, "rotated"))).unwrap();
        let rotated = read_spool_delta(&record, &mut cursor).unwrap();
        assert_eq!(
            rotated
                .lines
                .iter()
                .map(|line| (line.seq, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(3, "rotated")]
        );
        assert_eq!(cursor.last_seq, 3);
        assert_eq!(cursor.offsets.len(), 1);
        assert!(cursor.offsets.contains_key(&next_segment));
    }

    #[test]
    fn job_ids_are_exactly_six_lowercase_base36_digits() {
        assert!(valid_job_id("j-09azzz"));
        for invalid in ["j-09AZZZ", "j-12345", "j-1234567", "x-123456", "j-12_456"] {
            assert!(!valid_job_id(invalid), "{invalid}");
        }
    }
}
