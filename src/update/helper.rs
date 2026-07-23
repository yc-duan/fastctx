//! Copied updater helper, verified installation, rollback, restart, and finalization.

use super::model::{
    NPMMIRROR_REGISTRY, NpmDriver, NpmMode, NpmProvenance, NpmVersionAuthority,
    OFFICIAL_NPM_REGISTRY, UpdatePlan, UpdateRequest,
};
use crate::control::apply::{AppliedBinarySync, synchronize_applied_binary};
use crate::control::paths::ControlPaths;
use crate::control::settings::UpdateSource;
use crate::control::transaction;
use fs2::FileExt;
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

/// Request path consumed by the first process launched after an update.
pub(crate) const UPDATE_FINALIZE_ENV: &str = "FASTCTX_UPDATE_FINALIZE";
/// Diagnostic passed only to a fallback TUI after an update failure.
pub(crate) const UPDATE_FAILURE_ENV: &str = "FASTCTX_UPDATE_FAILURE";
/// Private exit code telling the npm launcher to wait on its handoff marker.
pub(crate) const NPM_LAUNCHER_WAIT_EXIT_CODE: u8 = 75;

const REQUEST_SCHEMA_VERSION: u32 = 3;
const NPM_HANDOFF_SCHEMA_VERSION: u32 = 2;
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_RELEASE_ASSET_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 64 * 1024;
const MAX_EXTRACTED_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);
const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const STALE_UPDATE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Notice shown by the restarted TUI after finalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FinalizeNotice {
    /// Version that passed restart finalization.
    pub(crate) version: String,
    /// Whether the separately owned Apply runtime was synchronized.
    pub(crate) outcome: FinalizeOutcome,
}

/// Stable Apply-copy result attached to a successful product update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FinalizeOutcome {
    /// No separate copy needed a change.
    Updated,
    /// The owned Codex runtime copy advanced too.
    RuntimeUpdated,
    /// The product updated, but ownership checks kept the runtime copy untouched.
    RuntimeUnchanged(String),
}

/// Which process remains responsible for the foreground terminal after handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateStart {
    /// A direct-download parent waited for the helper and restarted TUI session to finish.
    Completed,
    /// The npm launcher must remain alive until its private handoff marker reaches a terminal state.
    NpmLauncherWait,
}

#[derive(Debug, Serialize)]
struct NpmLauncherHandoff<'a> {
    schema_version: u32,
    state: &'a str,
    helper_pid: u32,
    helper_executable: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

struct NpmHandoffGuard {
    path: PathBuf,
    helper_executable: PathBuf,
    succeeded: bool,
    detail: Option<String>,
}

struct UpdatedSession {
    child: Child,
    cleanup_directories: Vec<PathBuf>,
}

impl NpmHandoffGuard {
    fn for_request(request: &UpdateRequest) -> Option<Self> {
        let UpdatePlan::Npm { provenance, .. } = &request.plan else {
            return None;
        };
        Some(Self {
            path: provenance.handoff_file.clone(),
            helper_executable: request.helper_executable.clone(),
            succeeded: false,
            detail: None,
        })
    }
}

impl Drop for NpmHandoffGuard {
    fn drop(&mut self) {
        let state = if self.succeeded { "done" } else { "failed" };
        let _ = write_handoff(
            &self.path,
            &NpmLauncherHandoff {
                schema_version: NPM_HANDOFF_SCHEMA_VERSION,
                state,
                helper_pid: std::process::id(),
                helper_executable: &self.helper_executable,
                detail: self.detail.as_deref(),
            },
            false,
        );
    }
}

/// Copies the running binary out of its installation and starts the updater handoff.
pub(crate) fn begin_update(
    paths: &ControlPaths,
    plan: UpdatePlan,
    current_executable: &Path,
) -> Result<UpdateStart, String> {
    validate_plan(&plan)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("invalid embedded FastCtx version: {error}"))?;
    let target = Version::parse(plan.target_version())
        .map_err(|error| format!("invalid selected update version: {error}"))?;
    if target <= current {
        return Err(format!(
            "refusing to update from v{current} to non-newer v{target}"
        ));
    }
    let metadata = fs::symlink_metadata(current_executable).map_err(|error| {
        format!(
            "Cannot inspect the running FastCtx binary {}: {error}",
            crate::paths::display_path(current_executable)
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "automatic updates require the running FastCtx binary to be a regular file".to_string(),
        );
    }

    let update_dir = prepare_update_directory(paths)?;
    cleanup_stale_update_files(&update_dir);
    let nonce = unique_nonce();
    let helper_executable = update_dir.join(format!(
        "helper-{nonce}{}",
        if cfg!(windows) { ".exe" } else { "" }
    ));
    let request_path = update_dir.join(format!("request-{nonce}.json"));
    let health_file = update_dir.join(format!("health-{nonce}"));
    let helper_bytes = fs::read(current_executable).map_err(|error| {
        format!(
            "Cannot copy the running FastCtx binary {}: {error}",
            crate::paths::display_path(current_executable)
        )
    })?;
    transaction::atomic_replace(&helper_executable, &helper_bytes, Some(0o700), false)?;

    let npm_handoff = match &plan {
        UpdatePlan::Npm { provenance, .. } => Some(provenance.handoff_file.clone()),
        UpdatePlan::GithubRelease { .. } => None,
    };
    let request = UpdateRequest {
        schema_version: REQUEST_SCHEMA_VERSION,
        current_version: current.to_string(),
        plan,
        target_executable: current_executable.to_path_buf(),
        helper_executable: helper_executable.clone(),
        health_file,
    };
    if let Err(error) = write_request(&request_path, &request) {
        let _ = fs::remove_file(&helper_executable);
        return Err(error);
    }
    if let Some(handoff) = npm_handoff.as_deref()
        && let Err(error) = write_handoff(
            handoff,
            &NpmLauncherHandoff {
                schema_version: NPM_HANDOFF_SCHEMA_VERSION,
                state: "starting",
                helper_pid: 0,
                helper_executable: &helper_executable,
                detail: None,
            },
            true,
        )
    {
        let _ = fs::remove_file(&request_path);
        let _ = fs::remove_file(&helper_executable);
        return Err(error);
    }

    let spawned = Command::new(&helper_executable)
        .arg("update-helper")
        .arg("--request")
        .arg(&request_path)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    let mut helper_child = match spawned {
        Ok(child) => child,
        Err(error) => {
            if let Some(handoff) = npm_handoff.as_deref() {
                let _ = fs::remove_file(handoff);
            }
            let _ = fs::remove_file(&request_path);
            let _ = fs::remove_file(&helper_executable);
            return Err(format!(
                "Cannot start the FastCtx updater helper {}: {error}",
                crate::paths::display_path(&helper_executable)
            ));
        }
    };
    if let Some(handoff) = npm_handoff.as_deref() {
        if let Err(error) = write_handoff(
            handoff,
            &NpmLauncherHandoff {
                schema_version: NPM_HANDOFF_SCHEMA_VERSION,
                state: "running",
                helper_pid: helper_child.id(),
                helper_executable: &helper_executable,
                detail: None,
            },
            false,
        ) {
            let _ = helper_child.kill();
            let _ = helper_child.wait();
            let _ = fs::remove_file(handoff);
            let _ = fs::remove_file(&request_path);
            let _ = fs::remove_file(&helper_executable);
            return Err(error);
        }
        return Ok(UpdateStart::NpmLauncherWait);
    }

    let status = helper_child.wait();
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&request.health_file);
    let _ = fs::remove_file(&helper_executable);
    let status = status.map_err(|error| {
        format!(
            "Cannot wait for the FastCtx updater helper {}: {error}",
            crate::paths::display_path(&helper_executable)
        )
    })?;
    if !status.success() {
        return Err(format!(
            "FastCtx updater helper exited with {}",
            status_label(status)
        ));
    }
    Ok(UpdateStart::Completed)
}

/// Removes rename-old artifacts once no previous Windows process still maps them.
pub(crate) fn cleanup_replaced_binaries(paths: &ControlPaths) {
    if let Ok(current) = std::env::current_exe() {
        crate::control::leftovers::cleanup_stale_binary_siblings(&current);
    }
    crate::control::leftovers::cleanup_stale_binary_siblings(&paths.installed_binary);
}

/// Executes the hidden updater-helper command after the TUI process has exited.
pub(crate) fn run_update_helper(
    paths: &ControlPaths,
    request_path: &Path,
    parent_pid: u32,
) -> Result<(), String> {
    let request = load_request(paths, request_path)?;
    if request.current_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "update request expects helper v{}, but v{} is running",
            request.current_version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    let helper = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the updater helper: {error}"))?;
    if !same_existing_path(&helper, &request.helper_executable) {
        return Err("update request does not name the running helper".to_string());
    }
    let mut handoff = NpmHandoffGuard::for_request(&request);
    let result = run_update_helper_inner(paths, request_path, parent_pid, &request);
    if let Some(handoff) = handoff.as_mut() {
        match &result {
            Ok(()) => handoff.succeeded = true,
            Err(error) => handoff.detail = Some(first_nonempty_line(error)),
        }
    }
    result
}

fn run_update_helper_inner(
    paths: &ControlPaths,
    request_path: &Path,
    parent_pid: u32,
    request: &UpdateRequest,
) -> Result<(), String> {
    if matches!(&request.plan, UpdatePlan::Npm { .. }) {
        wait_for_parent_exit(parent_pid)?;
    }
    let update_dir = update_directory(paths);
    let lock = open_private_lock(&update_dir.join("update.lock"))?;
    if let Err(error) = FileExt::try_lock_exclusive(&lock) {
        let error = format!("another FastCtx update is already running: {error}");
        eprintln!("FastCtx update did not start: {error}");
        cleanup_request_files(request_path, request);
        eprintln!("Reopening the previous FastCtx version…");
        let session = restart_fallback(request, &error)?;
        return finish_failed_update(error, wait_for_session(session, "previous FastCtx"));
    }

    println!(
        "Updating FastCtx v{} → v{}…",
        request.current_version,
        request.plan.target_version()
    );
    let result = match &request.plan {
        UpdatePlan::Npm {
            provenance,
            target_version,
            registry,
            ..
        } => apply_npm_update(
            paths,
            request_path,
            request,
            provenance,
            target_version,
            registry,
        ),
        UpdatePlan::GithubRelease { .. } => apply_release_update(paths, request_path, request),
    };
    let session = match result {
        Ok(session) => session,
        Err(error) => {
            eprintln!("FastCtx update failed: {error}");
            cleanup_request_files(request_path, request);
            eprintln!("Reopening the previous FastCtx version…");
            let fallback = restart_fallback(request, &error).map_err(|restart_error| {
                format!(
                    "{error}. The previous version remains installed, but could not be reopened: {restart_error}"
                )
            })?;
            drop(lock);
            return finish_failed_update(error, wait_for_session(fallback, "previous FastCtx"));
        }
    };
    drop(lock);
    wait_for_session(session, "updated FastCtx")
}

/// Finalizes the stable Apply copy, signals health, and supplies a TUI notice.
pub(crate) fn finalize_update(
    paths: &ControlPaths,
    request_path: &Path,
) -> Result<FinalizeNotice, String> {
    let request = load_request(paths, request_path)?;
    if request.plan.target_version() != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "updated process is v{}, but the request requires v{}",
            env!("CARGO_PKG_VERSION"),
            request.plan.target_version()
        ));
    }
    let current_executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the updated FastCtx binary: {error}"))?;
    match &request.plan {
        UpdatePlan::GithubRelease { .. } => {
            if !same_existing_path(&current_executable, &request.target_executable) {
                return Err(
                    "the updated GitHub Release started from an unexpected path".to_string()
                );
            }
        }
        UpdatePlan::Npm { provenance, .. } => {
            if std::env::var("FASTCTX_NPM_PACKAGE").ok().as_deref()
                != Some(provenance.package.as_str())
            {
                return Err("the updated npm launcher reported a different package".to_string());
            }
        }
    }

    // The product update is healthy once the exact new binary has passed provenance and version
    // checks. Signal before the optional Apply-copy sync so a later health-file failure can never
    // make the helper roll back the product after mutating that separate owned copy (2026-07-17).
    write_health_file(&request.health_file)?;
    let outcome = match synchronize_applied_binary(paths, &current_executable) {
        Ok(AppliedBinarySync::Updated) => FinalizeOutcome::RuntimeUpdated,
        Ok(AppliedBinarySync::NotApplied | AppliedBinarySync::Unchanged) => {
            FinalizeOutcome::Updated
        }
        Err(error) => FinalizeOutcome::RuntimeUnchanged(error),
    };
    Ok(FinalizeNotice {
        version: request.plan.target_version().to_string(),
        outcome,
    })
}

fn apply_release_update(
    paths: &ControlPaths,
    request_path: &Path,
    request: &UpdateRequest,
) -> Result<UpdatedSession, String> {
    let UpdatePlan::GithubRelease {
        target_version,
        archive_name,
        archive_url,
        checksums_url,
    } = &request.plan
    else {
        unreachable!("release updater received npm plan");
    };
    let archive = download(archive_url, MAX_RELEASE_ASSET_BYTES)?;
    let checksums = download(checksums_url, MAX_CHECKSUM_BYTES)?;
    verify_release_archive(archive_name, &archive, &checksums)?;
    let extracted = extract_release_binary(archive_name, &archive)?;
    probe_binary(paths, &extracted.bytes, target_version, extracted.unix_mode)?;

    let child = replace_release_with_rollback(
        &request.target_executable,
        &extracted.bytes,
        Some(extracted.unix_mode),
        || {
            let mut child = spawn_release_restart(&request.target_executable, request_path)?;
            if let Err(error) = wait_for_health(&mut child, &request.health_file) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(child)
        },
    )?;
    cleanup_request_files(request_path, request);
    Ok(UpdatedSession {
        child,
        cleanup_directories: Vec::new(),
    })
}

fn apply_npm_update(
    paths: &ControlPaths,
    request_path: &Path,
    request: &UpdateRequest,
    provenance: &NpmProvenance,
    target_version: &str,
    registry: &str,
) -> Result<UpdatedSession, String> {
    let cache = create_private_subdirectory(&update_directory(paths), "npm-update")?;
    match provenance.mode {
        NpmMode::Global => {
            let operation = || {
                install_npm_version(provenance, target_version, registry, &cache)?;
                let mut child = spawn_npm_launcher(provenance, request_path)?;
                if let Err(error) = wait_for_health(&mut child, &request.health_file) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
                Ok(child)
            };
            let rollback = || {
                let rollback_cache =
                    create_private_subdirectory(&update_directory(paths), "npm-rollback")?;
                let result = install_npm_version(
                    provenance,
                    &request.current_version,
                    registry,
                    &rollback_cache,
                );
                let _ = fs::remove_dir_all(&rollback_cache);
                result
            };
            let result = run_with_npm_rollback(&request.current_version, operation, rollback);
            let child = match result {
                Ok(child) => child,
                Err(error) => {
                    let _ = fs::remove_dir_all(&cache);
                    return Err(error);
                }
            };
            cleanup_request_files(request_path, request);
            Ok(UpdatedSession {
                child,
                cleanup_directories: vec![cache],
            })
        }
        NpmMode::Exec => {
            let mut child =
                spawn_npm_exec(provenance, target_version, registry, &cache, request_path)?;
            if let Err(error) = wait_for_health(&mut child, &request.health_file) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&cache);
                return Err(error);
            }
            cleanup_request_files(request_path, request);
            Ok(UpdatedSession {
                child,
                cleanup_directories: vec![cache],
            })
        }
    }
}

fn run_with_npm_rollback<T>(
    current_version: &str,
    operation: impl FnOnce() -> Result<T, String>,
    rollback: impl FnOnce() -> Result<(), String>,
) -> Result<T, String> {
    match operation() {
        Ok(value) => Ok(value),
        Err(error) => match rollback() {
            Ok(()) => Err(format!("{error}; npm restored v{current_version}")),
            Err(rollback_error) => Err(format!(
                "{error}; npm rollback also failed: {rollback_error}"
            )),
        },
    }
}

fn finish_failed_update(
    update_error: String,
    fallback_result: Result<(), String>,
) -> Result<(), String> {
    match fallback_result {
        Ok(()) => Err(update_error),
        Err(fallback_error) => Err(format!("{update_error}; {fallback_error}")),
    }
}

fn install_npm_version(
    provenance: &NpmProvenance,
    version: &str,
    registry: &str,
    cache: &Path,
) -> Result<(), String> {
    let spec = format!("{}@{version}", provenance.package);
    let status = super::npm_invocation::command(provenance)
        .args(npm_install_arguments(&spec, registry, cache))
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env_remove("NPM_CONFIG_OFFLINE")
        .env_remove("npm_config_offline")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("cannot start npm install for {spec}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "npm install {spec} exited with {}",
            status_label(status)
        ))
    }
}

fn npm_install_arguments(spec: &str, registry: &str, cache: &Path) -> Vec<OsString> {
    vec![
        OsString::from("install"),
        OsString::from("--global"),
        OsString::from(spec),
        OsString::from("--prefer-online"),
        OsString::from("--registry"),
        OsString::from(registry),
        OsString::from("--cache"),
        cache.as_os_str().to_os_string(),
        OsString::from("--ignore-scripts"),
        OsString::from("--no-audit"),
        OsString::from("--no-fund"),
        OsString::from("--fetch-retries"),
        OsString::from("2"),
        OsString::from("--fetch-timeout"),
        OsString::from("30000"),
        OsString::from("--loglevel"),
        OsString::from("notice"),
    ]
}

fn spawn_npm_exec(
    provenance: &NpmProvenance,
    version: &str,
    registry: &str,
    cache: &Path,
    request_path: &Path,
) -> Result<Child, String> {
    let spec = format!("{}@{version}", provenance.package);
    super::npm_invocation::command(provenance)
        .args([
            "exec",
            "--yes",
            "--prefer-online",
            "--registry",
            registry,
            "--cache",
        ])
        .arg(cache)
        .arg("--package")
        .arg(&spec)
        .args([
            "--fetch-retries",
            "2",
            "--fetch-timeout",
            "30000",
            "--loglevel",
            "notice",
            "--",
            "fastctx",
        ])
        .env(UPDATE_FINALIZE_ENV, request_path)
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env_remove("NPM_CONFIG_OFFLINE")
        .env_remove("npm_config_offline")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start npm exec for {spec}: {error}"))
}

fn spawn_npm_launcher(provenance: &NpmProvenance, request_path: &Path) -> Result<Child, String> {
    Command::new(&provenance.node)
        .arg(&provenance.launcher)
        .env(UPDATE_FINALIZE_ENV, request_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot restart the npm launcher: {error}"))
}

fn spawn_release_restart(target: &Path, request_path: &Path) -> Result<Child, String> {
    Command::new(target)
        .env(UPDATE_FINALIZE_ENV, request_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot restart the updated FastCtx binary: {error}"))
}

fn restart_fallback(request: &UpdateRequest, failure: &str) -> Result<UpdatedSession, String> {
    let mut command = match &request.plan {
        UpdatePlan::Npm { provenance, .. } => {
            let mut command = Command::new(&provenance.node);
            command.arg(&provenance.launcher);
            command
        }
        UpdatePlan::GithubRelease { .. } => Command::new(&request.target_executable),
    };
    let child = command
        .env("FASTCTX_DISABLE_UPDATE_CHECK", "1")
        .env(UPDATE_FAILURE_ENV, first_nonempty_line(failure))
        .env_remove(UPDATE_FINALIZE_ENV)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            format!(
                "cannot restart {} after update failure: {error}",
                crate::paths::display_path(&request.target_executable)
            )
        })?;
    Ok(UpdatedSession {
        child,
        cleanup_directories: Vec::new(),
    })
}

fn download(url: &str, maximum: u64) -> Result<Vec<u8>, String> {
    validate_release_url(url)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(DOWNLOAD_TIMEOUT)
        .timeout_connect(DOWNLOAD_TIMEOUT)
        .timeout_read(DOWNLOAD_TIMEOUT)
        .timeout_write(DOWNLOAD_TIMEOUT)
        .redirects(5)
        .build();
    let response = match agent
        .get(url)
        .set("Accept", "application/octet-stream")
        .set("User-Agent", concat!("fastctx/", env!("CARGO_PKG_VERSION")))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            return Err(format!("release download returned HTTP {status}"));
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(format!("release download could not connect: {error}"));
        }
    };
    validate_download_response_url(response.get_url())?;
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        && length > maximum
    {
        return Err(format!("release download reported unsafe length {length}"));
    }
    read_limited(response.into_reader(), maximum)
}

fn verify_release_archive(
    archive_name: &str,
    archive: &[u8],
    checksum_bytes: &[u8],
) -> Result<(), String> {
    let checksum_source = std::str::from_utf8(checksum_bytes)
        .map_err(|error| format!("SHA256SUMS is not UTF-8: {error}"))?;
    let mut filenames = BTreeSet::new();
    let mut expected = None;
    for line in checksum_source
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let bytes = line.as_bytes();
        if bytes.len() < 67
            || !bytes[..64].iter().all(|byte| byte.is_ascii_hexdigit())
            || bytes[64] != b' '
            || !matches!(bytes[65], b' ' | b'*')
        {
            return Err("SHA256SUMS has an invalid sha256sum-compatible line".to_string());
        }
        let filename = &line[66..];
        if filename.is_empty()
            || filename.contains('/')
            || filename.contains('\\')
            || !filenames.insert(filename.to_string())
        {
            return Err("SHA256SUMS contains an invalid or duplicate filename".to_string());
        }
        if filename == archive_name {
            expected = Some(&line[..64]);
        }
    }
    let expected = expected.ok_or_else(|| format!("SHA256SUMS does not contain {archive_name}"))?;
    let actual = sha256_hex(archive);
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(format!(
            "release archive {archive_name} SHA-256 does not match SHA256SUMS"
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct ExtractedBinary {
    bytes: Vec<u8>,
    unix_mode: u32,
}

fn extract_release_binary(archive_name: &str, bytes: &[u8]) -> Result<ExtractedBinary, String> {
    if archive_name.ends_with(".zip") {
        extract_zip_binary(bytes)
    } else if archive_name.ends_with(".tar.gz") {
        extract_tar_gz_binary(bytes)
    } else {
        Err(format!(
            "release archive {archive_name} has an unsupported format"
        ))
    }
}

fn extract_zip_binary(bytes: &[u8]) -> Result<ExtractedBinary, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("release zip is invalid: {error}"))?;
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    let mut binary = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("release zip entry {index} is invalid: {error}"))?;
        let name = validate_flat_archive_name(entry.name())?.to_string();
        if !names.insert(name.clone()) {
            return Err(format!("release zip contains duplicate entry {name}"));
        }
        if entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err(format!("release zip entry {name} is not a regular file"));
        }
        let contents = read_archive_entry(&mut entry, &mut total)?;
        if name == "fastctx.exe" {
            binary = Some(ExtractedBinary {
                bytes: contents,
                unix_mode: 0o755,
            });
        }
    }
    validate_archive_contents(&names, "fastctx.exe")?;
    binary.ok_or_else(|| "release zip does not contain fastctx.exe".to_string())
}

fn extract_tar_gz_binary(bytes: &[u8]) -> Result<ExtractedBinary, String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("release tar.gz is invalid: {error}"))?;
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    let mut binary = None;
    for (index, entry) in entries.enumerate() {
        let mut entry =
            entry.map_err(|error| format!("release tar.gz entry {index} is invalid: {error}"))?;
        if !entry.header().entry_type().is_file() {
            return Err(format!(
                "release tar.gz entry {index} is not a regular file"
            ));
        }
        let path = entry.path().map_err(|error| {
            format!("release tar.gz entry {index} has an invalid path: {error}")
        })?;
        let name = path
            .to_str()
            .ok_or_else(|| format!("release tar.gz entry {index} has a non-UTF-8 path"))?;
        let name = validate_flat_archive_name(name)?.to_string();
        if !names.insert(name.clone()) {
            return Err(format!("release tar.gz contains duplicate entry {name}"));
        }
        let mode =
            entry.header().mode().map_err(|error| {
                format!("release tar.gz entry {name} has no valid mode: {error}")
            })? & 0o777;
        let contents = read_archive_entry(&mut entry, &mut total)?;
        if name == "fastctx" {
            if mode & 0o111 == 0 {
                return Err("release tar.gz fastctx entry is not executable".to_string());
            }
            binary = Some(ExtractedBinary {
                bytes: contents,
                unix_mode: mode,
            });
        }
    }
    validate_archive_contents(&names, "fastctx")?;
    binary.ok_or_else(|| "release tar.gz does not contain fastctx".to_string())
}

fn validate_flat_archive_name(name: &str) -> Result<&str, String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
    {
        return Err(format!(
            "release archive entry {name:?} is not a flat safe filename"
        ));
    }
    Ok(name)
}

fn validate_archive_contents(names: &BTreeSet<String>, binary_name: &str) -> Result<(), String> {
    let expected = BTreeSet::from([
        binary_name.to_string(),
        "LICENSE-MIT".to_string(),
        "LICENSE-APACHE".to_string(),
        "NOTICE".to_string(),
        "THIRD_PARTY_LICENSES.md".to_string(),
    ]);
    if names != &expected {
        let missing = expected.difference(names).cloned().collect::<Vec<_>>();
        let extra = names.difference(&expected).cloned().collect::<Vec<_>>();
        return Err(format!(
            "release archive contents do not match the distribution contract (missing: {}; extra: {})",
            if missing.is_empty() {
                "none".to_string()
            } else {
                missing.join(", ")
            },
            if extra.is_empty() {
                "none".to_string()
            } else {
                extra.join(", ")
            }
        ));
    }
    Ok(())
}

fn read_archive_entry(reader: &mut impl Read, total: &mut u64) -> Result<Vec<u8>, String> {
    let remaining = MAX_EXTRACTED_ARCHIVE_BYTES.saturating_sub(*total);
    let mut contents = Vec::new();
    reader
        .take(remaining + 1)
        .read_to_end(&mut contents)
        .map_err(|error| format!("cannot read release archive entry: {error}"))?;
    if contents.len() as u64 > remaining {
        return Err(format!(
            "release archive expands beyond the {MAX_EXTRACTED_ARCHIVE_BYTES}-byte safety limit"
        ));
    }
    *total += contents.len() as u64;
    Ok(contents)
}

fn probe_binary(
    paths: &ControlPaths,
    bytes: &[u8],
    version: &str,
    unix_mode: u32,
) -> Result<(), String> {
    let probe = update_directory(paths).join(format!(
        "probe-{}{}",
        unique_nonce(),
        if cfg!(windows) { ".exe" } else { "" }
    ));
    transaction::atomic_replace(&probe, bytes, Some(unix_mode), false)?;
    let result = probe_version(&probe, version);
    let cleanup = fs::remove_file(&probe);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(format!(
            "downloaded binary passed validation, but its probe file could not be removed: {error}"
        )),
        (Err(error), _) => Err(error),
    }
}

fn probe_version(binary: &Path, version: &str) -> Result<(), String> {
    let mut command = crate::process_policy::noninteractive_command(binary);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        format!(
            "cannot execute downloaded binary {}: {error}",
            crate::paths::display_path(binary)
        )
    })?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < VERSION_PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("downloaded binary version probe timed out".to_string());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for downloaded binary: {error}"));
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(|error| format!("cannot read downloaded binary version: {error}"))?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)
            .map_err(|error| format!("cannot read downloaded binary diagnostics: {error}"))?;
    }
    let expected = format!("fastctx {version}");
    if !status.success() || stdout.trim() != expected {
        return Err(format!(
            "downloaded binary failed its version probe: expected {expected:?}, got {:?} ({})",
            stdout.trim(),
            first_nonempty_line(&stderr)
        ));
    }
    Ok(())
}

fn replace_release_with_rollback<T>(
    target: &Path,
    replacement: &[u8],
    mode: Option<u32>,
    confirm: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let original = transaction::read_snapshot(target)?
        .ok_or_else(|| "the release binary disappeared before update".to_string())?;
    transaction::atomic_replace(target, replacement, mode, true)?;
    match confirm() {
        Ok(value) => Ok(value),
        Err(error) => {
            transaction::atomic_replace(target, &original, mode, true).map_err(
                |rollback_error| {
                    format!("{error}; cannot restore the previous release binary: {rollback_error}")
                },
            )?;
            Err(format!("{error}; the previous binary was restored"))
        }
    }
}

fn wait_for_session(mut session: UpdatedSession, label: &str) -> Result<(), String> {
    let status = session
        .child
        .wait()
        .map_err(|error| format!("cannot wait for {label}: {error}"));
    for directory in session.cleanup_directories {
        let _ = fs::remove_dir_all(directory);
    }
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with {}", status_label(status)))
    }
}

fn wait_for_health(child: &mut Child, health_file: &Path) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match fs::read(health_file) {
            Ok(bytes) if bytes == b"ok\n" => return Ok(()),
            Ok(_) => return Err("updated process wrote an invalid health signal".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot read update health signal: {error}")),
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for restarted FastCtx: {error}"))?
        {
            return Err(format!(
                "updated FastCtx exited before finalization ({})",
                status_label(status)
            ));
        }
        if started.elapsed() >= HEALTH_TIMEOUT {
            return Err(format!(
                "updated FastCtx did not finalize within {} seconds",
                HEALTH_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write_health_file(path: &Path) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot create update health signal: {error}"))?;
    file.write_all(b"ok\n")
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Cannot persist update health signal: {error}"))
}

fn write_handoff(
    path: &Path,
    handoff: &NpmLauncherHandoff<'_>,
    create_new: bool,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(handoff)
        .map_err(|error| format!("Cannot encode npm update handoff: {error}"))?;
    if !create_new {
        return transaction::atomic_replace(path, &bytes, Some(0o600), false)
            .map_err(|error| format!("Cannot update npm launcher handoff: {error}"));
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        format!(
            "Cannot create npm launcher handoff {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Cannot persist npm launcher handoff: {error}"))
}

fn write_request(path: &Path, request: &UpdateRequest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(request)
        .map_err(|error| format!("Cannot encode update request: {error}"))?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        format!(
            "Cannot create update request {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Cannot persist update request: {error}"))
}

fn load_request(paths: &ControlPaths, path: &Path) -> Result<UpdateRequest, String> {
    let update_dir = prepare_update_directory(paths)?;
    validate_update_child_path(&update_dir, path, "request-", Some("json"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Cannot inspect update request: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_REQUEST_BYTES
    {
        return Err("update request is not a safe regular file".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("Cannot read update request: {error}"))?;
    let request: UpdateRequest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse update request: {error}"))?;
    if request.schema_version != REQUEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported update request schema {}",
            request.schema_version
        ));
    }
    validate_plan(&request.plan)?;
    Version::parse(&request.current_version)
        .map_err(|error| format!("update request has invalid current version: {error}"))?;
    if !request.target_executable.is_absolute() {
        return Err("update request target is not absolute".to_string());
    }
    validate_update_child_path(&update_dir, &request.helper_executable, "helper-", None)?;
    validate_update_child_path(&update_dir, &request.health_file, "health-", None)?;
    if let UpdatePlan::Npm { provenance, .. } = &request.plan {
        validate_update_child_path(
            &update_dir,
            &provenance.handoff_file,
            "npm-launcher-",
            Some("handoff"),
        )?;
        let expected_handoff_name = format!("npm-launcher-{}.handoff", provenance.launcher_pid);
        if provenance
            .handoff_file
            .file_name()
            .and_then(|name| name.to_str())
            != Some(expected_handoff_name.as_str())
        {
            return Err("update request has a mismatched npm launcher handoff".to_string());
        }
    }
    Ok(request)
}

fn validate_plan(plan: &UpdatePlan) -> Result<(), String> {
    let target = Version::parse(plan.target_version())
        .map_err(|error| format!("update plan has invalid target version: {error}"))?;
    if !target.pre.is_empty() || !target.build.is_empty() {
        return Err("update plan does not target a stable release version".to_string());
    }
    match plan {
        UpdatePlan::Npm {
            provenance,
            target_version,
            registry,
            source_name,
            discovery,
        } => {
            if !matches!(provenance.package.as_str(), "fastctx" | "codex-fastctx") {
                return Err("update plan has an unsupported npm package".to_string());
            }
            for (name, path) in [
                ("Node.js", &provenance.node),
                ("npm CLI", &provenance.npm_cli),
                ("npm launcher", &provenance.launcher),
                ("npm handoff", &provenance.handoff_file),
            ] {
                if !path.is_absolute() {
                    return Err(format!("update plan has a relative {name} path"));
                }
            }
            validate_npm_driver(provenance)?;
            if provenance.launcher_pid == 0 {
                return Err("update plan has an invalid npm launcher process id".to_string());
            }
            validate_registry_url(registry)?;
            if discovery.target_version != *target_version {
                return Err(
                    "update plan target does not match its npm discovery evidence".to_string(),
                );
            }
            let source_policy = UpdateSource::parse(&discovery.source_policy)
                .ok_or_else(|| "update plan has an unsupported npm source policy".to_string())?;
            if discovery.selected_registry.as_deref() != Some(registry)
                || discovery.selected_source.as_deref() != Some(source_name)
            {
                return Err(
                    "update plan source does not match its npm discovery evidence".to_string(),
                );
            }
            let selected_probe = discovery
                .probes
                .iter()
                .find(|probe| {
                    probe.registry == *registry
                        && probe.source_name == *source_name
                        && probe.is_ready()
                })
                .ok_or_else(|| {
                    "update plan source did not pass the exact two-package preflight".to_string()
                })?;
            validate_registry_url(&selected_probe.registry)?;
            if discovery.platform_package != expected_npm_platform_package().unwrap_or_default() {
                return Err("update plan names an npm package for another platform".to_string());
            }
            validate_selected_source_policy(
                source_policy,
                discovery.configured_registry.as_deref(),
                registry,
            )?;
            validate_npm_version_authority(discovery)?;
        }
        UpdatePlan::GithubRelease {
            archive_name,
            archive_url,
            checksums_url,
            ..
        } => {
            if archive_name != expected_release_archive_name().unwrap_or_default() {
                return Err("update plan names an archive for another platform".to_string());
            }
            validate_release_url(archive_url)?;
            validate_release_url(checksums_url)?;
            let base = format!("https://github.com/yc-duan/fastctx/releases/download/v{target}");
            if archive_url != &format!("{base}/{archive_name}")
                || checksums_url != &format!("{base}/SHA256SUMS")
            {
                return Err(
                    "update plan URLs do not match its exact target tag and platform archive"
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

fn validate_npm_driver(provenance: &NpmProvenance) -> Result<(), String> {
    match provenance.driver {
        NpmDriver::NodeScript => {
            let is_npm_cli = provenance
                .npm_cli
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches!(
                        name.to_ascii_lowercase().as_str(),
                        "npm-cli.js" | "npm-cli.cjs" | "npm-cli.mjs"
                    )
                });
            if !is_npm_cli {
                return Err("update plan has a non-npm Node.js entry point".to_string());
            }
        }
        NpmDriver::Executable => {
            if cfg!(windows)
                && provenance
                    .npm_cli
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(extension.to_ascii_lowercase().as_str(), "cmd" | "bat")
                    })
            {
                return Err(
                    "update plan uses a shell script as an executable npm driver".to_string(),
                );
            }
        }
    }
    Ok(())
}

fn validate_registry_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("invalid npm registry URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().ends_with('/')
        || url.to_string() != value
    {
        return Err(
            "update plan has an unsupported or non-normalized npm registry URL".to_string(),
        );
    }
    Ok(())
}

fn validate_selected_source_policy(
    policy: UpdateSource,
    configured_registry: Option<&str>,
    selected_registry: &str,
) -> Result<(), String> {
    let allowed = match policy {
        UpdateSource::Auto => {
            configured_registry == Some(selected_registry)
                || matches!(
                    selected_registry,
                    OFFICIAL_NPM_REGISTRY | NPMMIRROR_REGISTRY
                )
        }
        UpdateSource::NpmConfig => configured_registry == Some(selected_registry),
        UpdateSource::Official => selected_registry == OFFICIAL_NPM_REGISTRY,
        UpdateSource::Npmmirror => selected_registry == NPMMIRROR_REGISTRY,
    };
    if allowed {
        Ok(())
    } else {
        Err("update plan selected a registry outside its source policy".to_string())
    }
}

fn validate_npm_version_authority(discovery: &super::model::NpmDiscovery) -> Result<(), String> {
    let parse = |label: &str, value: &str| {
        let version = Version::parse(value)
            .map_err(|error| format!("update plan has invalid {label} version: {error}"))?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(format!("update plan has a non-stable {label} version"));
        }
        Ok(version)
    };
    let target = parse("target", &discovery.target_version)?;
    let github = discovery
        .github_version
        .as_deref()
        .map(|value| parse("GitHub", value))
        .transpose()?;
    let official = discovery
        .official_version
        .as_deref()
        .map(|value| parse("official npm", value))
        .transpose()?;
    let authoritative = [github, official].into_iter().flatten().max();
    match authoritative {
        Some(version)
            if discovery.authority == NpmVersionAuthority::Official && version == target =>
        {
            Ok(())
        }
        Some(_) => Err(
            "update plan target does not match the highest official version authority".to_string(),
        ),
        None if discovery.authority == NpmVersionAuthority::MirrorFallback => {
            let mirror_max = discovery
                .probes
                .iter()
                .filter_map(|probe| probe.latest_version.as_deref())
                .map(|value| parse("mirror", value))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .max();
            if mirror_max.as_ref() == Some(&target) {
                Ok(())
            } else {
                Err("update plan mirror fallback does not match its source evidence".to_string())
            }
        }
        None => {
            Err("update plan claims official authority without an official version".to_string())
        }
    }
}

fn prepare_update_directory(paths: &ControlPaths) -> Result<PathBuf, String> {
    if let Ok(metadata) = fs::symlink_metadata(&paths.fastctx_dir)
        && metadata.file_type().is_symlink()
    {
        return Err("automatic updates refuse a symbolic-link ~/.fastctx directory".to_string());
    }
    fs::create_dir_all(&paths.fastctx_dir).map_err(|error| {
        format!(
            "Cannot create {}: {error}",
            crate::paths::display_path(&paths.fastctx_dir)
        )
    })?;
    let update_dir = update_directory(paths);
    match create_private_directory(&update_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&update_dir)
                .map_err(|error| format!("Cannot inspect update directory: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("automatic update directory is not a regular directory".to_string());
            }
        }
        Err(error) => return Err(format!("Cannot create update directory: {error}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&update_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Cannot secure update directory: {error}"))?;
    }
    Ok(update_dir)
}

fn update_directory(paths: &ControlPaths) -> PathBuf {
    paths.fastctx_dir.join("update")
}

fn create_private_subdirectory(parent: &Path, purpose: &str) -> Result<PathBuf, String> {
    for _ in 0..32 {
        let path = parent.join(format!("{purpose}-{}", unique_nonce()));
        match create_private_directory(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot create isolated npm cache: {error}")),
        }
    }
    Err("cannot allocate a unique isolated npm cache".to_string())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

fn validate_update_child_path(
    update_dir: &Path,
    path: &Path,
    prefix: &str,
    extension: Option<&str>,
) -> Result<(), String> {
    if !path.is_absolute() || path.parent() != Some(update_dir) {
        return Err("update request references a path outside the update directory".to_string());
    }
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return Err("update request references a non-UTF-8 path".to_string());
    };
    if !filename.starts_with(prefix)
        || extension.is_some_and(|extension| {
            path.extension().and_then(|value| value.to_str()) != Some(extension)
        })
    {
        return Err("update request references an unexpected filename".to_string());
    }
    Ok(())
}

fn open_private_lock(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| format!("Cannot open update lock: {error}"))
}

fn cleanup_request_files(request_path: &Path, request: &UpdateRequest) {
    let _ = fs::remove_file(&request.health_file);
    let _ = fs::remove_file(request_path);
}

fn cleanup_stale_update_files(update_dir: &Path) {
    let Ok(entries) = fs::read_dir(update_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if ![
            "helper-",
            "request-",
            "health-",
            "probe-",
            "npm-update-",
            "npm-rollback-",
            "npm-launcher-",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= STALE_UPDATE_AGE);
        if !stale {
            continue;
        }
        if metadata.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fn validate_release_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("invalid release URL: {error}"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url
            .path()
            .starts_with("/yc-duan/fastctx/releases/download/")
    {
        return Err("release URL is outside yc-duan/fastctx".to_string());
    }
    Ok(())
}

fn validate_download_response_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("invalid download URL: {error}"))?;
    let allowed = match url.host_str() {
        Some("github.com") => url
            .path()
            .starts_with("/yc-duan/fastctx/releases/download/"),
        Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com") => true,
        _ => false,
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || !allowed
    {
        return Err(format!("refusing release bytes from {value}"));
    }
    Ok(())
}

fn read_limited(mut reader: impl Read, maximum: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read release download: {error}"))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "release download exceeded the {maximum}-byte safety limit"
        ));
    }
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn unique_nonce() -> String {
    let sequence = FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}-{sequence}", std::process::id())
}

fn first_nonempty_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no diagnostics")
        .chars()
        .take(240)
        .collect()
}

fn status_label(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit code {code}"))
        .unwrap_or_else(|| "a termination signal".to_string())
}

fn same_existing_path(left: &Path, right: &Path) -> bool {
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(unix)]
fn wait_for_parent_exit(parent_pid: u32) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let result = unsafe { libc::kill(parent_pid as i32, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::EPERM) {
                return Err(format!("Cannot inspect exiting TUI process: {error}"));
            }
        }
        if started.elapsed() >= PARENT_EXIT_TIMEOUT {
            return Err("the original TUI process did not exit in time".to_string());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn wait_for_parent_exit(parent_pid: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, parent_pid) };
    if handle.is_null() {
        return Ok(());
    }
    let outcome = unsafe { WaitForSingleObject(handle, PARENT_EXIT_TIMEOUT.as_millis() as u32) };
    unsafe {
        CloseHandle(handle);
    }
    match outcome {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err("the original TUI process did not exit in time".to_string()),
        WAIT_FAILED => Err(format!(
            "Cannot wait for the original TUI process: {}",
            std::io::Error::last_os_error()
        )),
        value => Err(format!("unexpected parent wait result {value}")),
    }
}

fn expected_release_archive_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("fastctx-x86_64-pc-windows-msvc.zip"),
        ("linux", "x86_64") => Some("fastctx-x86_64-unknown-linux-gnu.tar.gz"),
        ("macos", "x86_64") => Some("fastctx-x86_64-apple-darwin.tar.gz"),
        ("macos", "aarch64") => Some("fastctx-aarch64-apple-darwin.tar.gz"),
        _ => None,
    }
}

fn expected_npm_platform_package() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("@fastctx/win32-x64"),
        ("linux", "x86_64") => Some("@fastctx/linux-x64"),
        ("macos", "x86_64") => Some("@fastctx/darwin-x64"),
        ("macos", "aarch64") => Some("@fastctx/darwin-arm64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NPM_HANDOFF_SCHEMA_VERSION, NpmLauncherHandoff, REQUEST_SCHEMA_VERSION,
        extract_release_binary, finish_failed_update, npm_install_arguments,
        replace_release_with_rollback, run_with_npm_rollback, sha256_hex,
        validate_download_response_url, validate_plan, verify_release_archive, write_handoff,
    };
    use crate::update::model::{
        NpmDiscovery, NpmDriver, NpmMode, NpmProvenance, NpmRegistryProbe, NpmVersionAuthority,
        OFFICIAL_NPM_REGISTRY, UpdatePlan,
    };
    use std::io::{Cursor, Write};

    #[test]
    fn aggregate_checksums_are_an_independent_exact_filename_oracle() {
        let archive = b"release archive";
        let digest = sha256_hex(archive);
        let sums = format!(
            "{}  another-platform.tar.gz\n{digest}  fastctx-test.zip\n",
            "0".repeat(64)
        );
        verify_release_archive("fastctx-test.zip", archive, sums.as_bytes()).unwrap();

        let wrong_name = format!("{digest}  another-file.zip\n");
        assert!(
            verify_release_archive("fastctx-test.zip", archive, wrong_name.as_bytes())
                .unwrap_err()
                .contains("does not contain")
        );
        assert!(
            verify_release_archive("fastctx-test.zip", b"tampered", sums.as_bytes())
                .unwrap_err()
                .contains("does not match")
        );
        let duplicate = format!("{digest}  fastctx-test.zip\n{digest}  fastctx-test.zip\n");
        assert!(
            verify_release_archive("fastctx-test.zip", archive, duplicate.as_bytes())
                .unwrap_err()
                .contains("duplicate")
        );
    }

    #[test]
    fn release_plan_rejects_cross_repository_urls() {
        let Some(archive_name) = super::expected_release_archive_name() else {
            return;
        };
        let plan = UpdatePlan::GithubRelease {
            target_version: "9.9.9".to_string(),
            archive_name: archive_name.to_string(),
            archive_url: format!(
                "https://github.com/attacker/project/releases/download/v9.9.9/{archive_name}"
            ),
            checksums_url:
                "https://github.com/attacker/project/releases/download/v9.9.9/SHA256SUMS"
                    .to_string(),
        };
        assert!(validate_plan(&plan).unwrap_err().contains("outside"));
    }

    #[test]
    fn release_redirects_accept_only_the_documented_github_asset_hosts() {
        for url in [
            "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/asset.zip",
            "https://release-assets.githubusercontent.com/github-production-release-asset/fixture",
            "https://objects.githubusercontent.com/github-production-release-asset/fixture",
        ] {
            validate_download_response_url(url).unwrap();
        }
        for url in [
            "http://release-assets.githubusercontent.com/fixture",
            "https://github.com/attacker/project/releases/download/v0.2.0/asset.zip",
            "https://github.example.com/fixture",
            "https://raw.githubusercontent.com/yc-duan/fastctx/main/fixture",
        ] {
            assert!(
                validate_download_response_url(url)
                    .unwrap_err()
                    .contains("refusing release bytes"),
                "{url}"
            );
        }
    }

    #[test]
    fn zip_and_tar_gz_extract_only_the_flat_distribution_contract() {
        let zip = make_zip(&release_entries("fastctx.exe"), 0o755);
        let extracted = extract_release_binary("fastctx-test.zip", &zip).unwrap();
        assert_eq!(extracted.bytes, b"release binary");

        let tar = make_tar_gz(&release_entries("fastctx"), 0o755);
        let extracted = extract_release_binary("fastctx-test.tar.gz", &tar).unwrap();
        assert_eq!(extracted.bytes, b"release binary");
        assert_eq!(extracted.unix_mode, 0o755);
    }

    #[test]
    fn archives_reject_traversal_duplicates_missing_licenses_and_non_executable_tar_binary() {
        let mut traversal = release_entries("fastctx.exe");
        traversal[0].0 = "../fastctx.exe";
        assert!(
            extract_release_binary("fastctx-test.zip", &make_zip(&traversal, 0o755))
                .unwrap_err()
                .contains("flat safe filename")
        );

        let mut duplicate = release_entries("fastctx");
        duplicate.push(("fastctx", b"second binary"));
        assert!(
            extract_release_binary("fastctx-test.tar.gz", &make_tar_gz(&duplicate, 0o755))
                .unwrap_err()
                .contains("duplicate")
        );

        let mut missing = release_entries("fastctx.exe");
        missing.retain(|(name, _)| *name != "NOTICE");
        assert!(
            extract_release_binary("fastctx-test.zip", &make_zip(&missing, 0o755))
                .unwrap_err()
                .contains("missing: NOTICE")
        );

        let tar = make_tar_gz(&release_entries("fastctx"), 0o644);
        assert!(
            extract_release_binary("fastctx-test.tar.gz", &tar)
                .unwrap_err()
                .contains("not executable")
        );
    }

    fn release_entries(binary_name: &'static str) -> Vec<(&'static str, &'static [u8])> {
        vec![
            (binary_name, b"release binary"),
            ("LICENSE-MIT", b"MIT"),
            ("LICENSE-APACHE", b"Apache"),
            ("NOTICE", b"Notice"),
            ("THIRD_PARTY_LICENSES.md", b"Third party"),
        ]
    }

    fn make_zip(entries: &[(&str, &[u8])], mode: u32) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            let options = zip::write::SimpleFileOptions::default().unix_permissions(mode);
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn make_tar_gz(entries: &[(&str, &[u8])], binary_mode: u32) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(if *name == "fastctx" {
                binary_mode
            } else {
                0o644
            });
            header.set_cksum();
            builder.append_data(&mut header, *name, *bytes).unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn npm_install_is_exact_script_free_and_uses_an_isolated_cache() {
        let arguments = npm_install_arguments(
            "fastctx@0.2.0",
            "https://registry.example.test/custom/",
            std::path::Path::new("/isolated/cache"),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        for expected in [
            "fastctx@0.2.0",
            "--global",
            "--prefer-online",
            "https://registry.example.test/custom/",
            "--cache",
            "/isolated/cache",
            "--ignore-scripts",
        ] {
            assert!(arguments.iter().any(|value| value == expected));
        }
        assert!(!arguments.iter().any(|value| value == "latest"));
        assert!(!arguments.iter().any(|value| value == "clean"));
    }

    #[test]
    fn npm_plan_requires_exact_main_and_platform_preflight_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let target_version = "0.2.0";
        let source_name = "official npm";
        let platform_package = super::expected_npm_platform_package().unwrap();
        let discovery = NpmDiscovery {
            source_policy: "official".to_string(),
            configured_registry: None,
            target_version: target_version.to_string(),
            authority: NpmVersionAuthority::Official,
            github_version: Some(target_version.to_string()),
            official_version: Some(target_version.to_string()),
            platform_package: platform_package.to_string(),
            probes: vec![NpmRegistryProbe {
                source_name: source_name.to_string(),
                registry: OFFICIAL_NPM_REGISTRY.to_string(),
                reachable: true,
                latest_version: Some(target_version.to_string()),
                main_package_ready: true,
                platform_package_ready: true,
                error: None,
                error_kind: None,
            }],
            selected_registry: Some(OFFICIAL_NPM_REGISTRY.to_string()),
            selected_source: Some(source_name.to_string()),
            selection_reason: "the configured source policy selected official npm".to_string(),
        };
        let mut plan = UpdatePlan::Npm {
            provenance: NpmProvenance {
                package: "fastctx".to_string(),
                mode: NpmMode::Global,
                node: temp.path().join("node"),
                driver: NpmDriver::NodeScript,
                npm_cli: temp.path().join("npm-cli.js"),
                launcher: temp.path().join("launcher.js"),
                launcher_pid: 42,
                handoff_file: temp.path().join("npm-launcher-42.handoff"),
            },
            target_version: target_version.to_string(),
            registry: OFFICIAL_NPM_REGISTRY.to_string(),
            source_name: source_name.to_string(),
            discovery: Box::new(discovery),
        };
        validate_plan(&plan).unwrap();
        assert_eq!(REQUEST_SCHEMA_VERSION, 3);
        let encoded = serde_json::to_value(&plan).unwrap();
        assert_eq!(encoded["provenance"]["driver"], "node-script");
        let decoded: UpdatePlan = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, plan);
        let mut invalid_driver = encoded;
        invalid_driver["provenance"]["driver"] = serde_json::Value::String("shell".to_string());
        assert!(serde_json::from_value::<UpdatePlan>(invalid_driver).is_err());

        let UpdatePlan::Npm { provenance, .. } = &mut plan else {
            unreachable!();
        };
        provenance.driver = NpmDriver::Executable;
        provenance.npm_cli = temp
            .path()
            .join(if cfg!(windows) { "npm.exe" } else { "npm" });
        validate_plan(&plan).unwrap();
        let UpdatePlan::Npm { provenance, .. } = &mut plan else {
            unreachable!();
        };
        provenance.driver = NpmDriver::NodeScript;
        provenance.npm_cli = temp.path().join("not-npm.js");
        assert_eq!(
            validate_plan(&plan).unwrap_err(),
            "update plan has a non-npm Node.js entry point"
        );
        let UpdatePlan::Npm {
            provenance,
            discovery,
            ..
        } = &mut plan
        else {
            unreachable!();
        };
        provenance.npm_cli = temp.path().join("npm-cli.js");
        discovery.probes[0].platform_package_ready = false;
        assert_eq!(
            validate_plan(&plan).unwrap_err(),
            "update plan source did not pass the exact two-package preflight"
        );
    }

    #[test]
    fn release_replacement_restores_old_bytes_when_restart_health_fails() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(if cfg!(windows) {
            "fastctx.exe"
        } else {
            "fastctx"
        });
        std::fs::write(&target, b"old release").unwrap();

        let error = replace_release_with_rollback(&target, b"new release", Some(0o755), || {
            assert_eq!(std::fs::read(&target).unwrap(), b"new release");
            Err::<(), _>("injected restart health failure".to_string())
        })
        .unwrap_err();
        assert!(error.contains("previous binary was restored"), "{error}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old release");

        replace_release_with_rollback(&target, b"healthy release", Some(0o755), || Ok(())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"healthy release");
    }

    #[test]
    fn npm_failure_always_attempts_exact_old_version_rollback() {
        let rollback_called = std::cell::Cell::new(false);
        let error = run_with_npm_rollback::<()>(
            "0.1.0",
            || Err("injected npm update failure".to_string()),
            || {
                rollback_called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(rollback_called.get());
        assert_eq!(error, "injected npm update failure; npm restored v0.1.0");

        let value = run_with_npm_rollback(
            "0.1.0",
            || Ok(42),
            || panic!("rollback must not run after a healthy update"),
        )
        .unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn reopening_the_old_tui_never_turns_an_update_failure_into_success() {
        assert_eq!(
            finish_failed_update("injected update failure".to_string(), Ok(())).unwrap_err(),
            "injected update failure"
        );
        assert_eq!(
            finish_failed_update(
                "injected update failure".to_string(),
                Err("previous FastCtx also failed".to_string()),
            )
            .unwrap_err(),
            "injected update failure; previous FastCtx also failed"
        );
    }

    #[test]
    fn stale_binary_cleanup_is_scoped_to_the_exact_target_name() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("fastctx.exe");
        let owned_old = temp.path().join(".fastctx.exe.fastctx-old-12.0");
        let another_binary = temp.path().join(".other.exe.fastctx-old-12.0");
        let lookalike = temp.path().join(".fastctx.exe.user-backup");
        for path in [&owned_old, &another_binary, &lookalike] {
            std::fs::write(path, b"fixture").unwrap();
        }

        crate::control::leftovers::cleanup_stale_binary_siblings(&target);
        assert!(!owned_old.exists());
        assert!(another_binary.exists());
        assert!(lookalike.exists());
    }

    #[test]
    fn npm_launcher_handoff_moves_atomically_to_a_terminal_state() {
        let temp = tempfile::tempdir().unwrap();
        let handoff = temp.path().join("npm-launcher-42.handoff");
        let helper = temp.path().join("helper.exe");
        write_handoff(
            &handoff,
            &NpmLauncherHandoff {
                schema_version: NPM_HANDOFF_SCHEMA_VERSION,
                state: "starting",
                helper_pid: 0,
                helper_executable: &helper,
                detail: None,
            },
            true,
        )
        .unwrap();
        let starting: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&handoff).unwrap()).unwrap();
        assert_eq!(starting["state"], "starting");

        write_handoff(
            &handoff,
            &NpmLauncherHandoff {
                schema_version: NPM_HANDOFF_SCHEMA_VERSION,
                state: "done",
                helper_pid: 42,
                helper_executable: &helper,
                detail: None,
            },
            false,
        )
        .unwrap();
        let done: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&handoff).unwrap()).unwrap();
        assert_eq!(done["state"], "done");
        assert_eq!(done["helper_pid"], 42);
    }
}
