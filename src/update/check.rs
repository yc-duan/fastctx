//! Installation-source detection, cached discovery, and classified update failures.

use super::cache::{self, CachedOutcome, CheckStatus};
use super::model::{
    CheckFailure, CheckFailureKind, NpmMode, NpmProvenance, StartupUpdate, UpdatePlan,
};
use crate::control::paths::ControlPaths;
use semver::Version;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

const GITHUB_LATEST_URL: &str = "https://github.com/yc-duan/fastctx/releases/latest";
const GITHUB_RELEASE_BASE: &str = "https://github.com/yc-duan/fastctx/releases/download";
const NPM_REGISTRY: &str = "https://registry.npmjs.org/";
const GITHUB_RELEASE_DISTRIBUTION: &str = "github-release";
const NPM_QUERY_TIMEOUT: Duration = Duration::from_secs(8);
const GITHUB_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_NPM_OUTPUT_BYTES: u64 = 1024 * 1024;
const NPM_MARKER_ENV: &str = "FASTCTX_NPM_LAUNCHER_VERSION";
const NPM_PACKAGE_ENV: &str = "FASTCTX_NPM_PACKAGE";
const NPM_MODE_ENV: &str = "FASTCTX_NPM_MODE";
const NODE_ENV: &str = "FASTCTX_NODE_EXECUTABLE";
const NPM_CLI_ENV: &str = "FASTCTX_NPM_CLI";
const NPM_LAUNCHER_ENV: &str = "FASTCTX_NPM_LAUNCHER";
const NPM_LAUNCHER_PID_ENV: &str = "FASTCTX_NPM_LAUNCHER_PID";
const NPM_HANDOFF_ENV: &str = "FASTCTX_NPM_HANDOFF";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
enum InstallChannel {
    Npm(NpmProvenance),
    GithubRelease,
    Unsupported,
}

/// Checks the authoritative channel, using a successful result cached for 24 hours.
pub(crate) fn check_for_update(paths: &ControlPaths) -> StartupUpdate {
    check_for_update_at(paths, false, SystemTime::now())
}

/// Bypasses the successful-result TTL for a user-requested Status recheck.
pub(crate) fn force_check_for_update(paths: &ControlPaths) -> StartupUpdate {
    check_for_update_at(paths, true, SystemTime::now())
}

/// Runs one update check off the TUI event loop.
pub(crate) fn spawn_update_check(
    paths: ControlPaths,
    force: bool,
) -> mpsc::Receiver<StartupUpdate> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            if force {
                force_check_for_update(&paths)
            } else {
                check_for_update(&paths)
            }
        })
        .unwrap_or_else(|_| StartupUpdate::Failed(structural("the update-check worker panicked")));
        let _ = sender.send(result);
    });
    receiver
}

/// Reads the last matching update attempt without touching the network or creating storage.
pub(crate) fn last_check_status(paths: &ControlPaths) -> CheckStatus {
    if update_check_disabled() {
        return CheckStatus {
            detail: "Automatic update checks are disabled by FASTCTX_DISABLE_UPDATE_CHECK."
                .to_string(),
        };
    }
    let current_executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return CheckStatus {
                detail: format!("Cannot identify this installation source: {error}"),
            };
        }
    };
    let current_version = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(error) => {
            return CheckStatus {
                detail: format!("The embedded FastCtx version is invalid: {error}"),
            };
        }
    };
    let channel = match detect_install_channel(
        paths,
        &current_executable,
        &|name| std::env::var_os(name),
        option_env!("FASTCTX_DISTRIBUTION") == Some(GITHUB_RELEASE_DISTRIBUTION),
    ) {
        Ok(channel) => channel,
        Err(error) => {
            return CheckStatus {
                detail: format!("Cannot identify this installation source: {error}"),
            };
        }
    };
    let Some(channel_key) = channel_key(&channel) else {
        return CheckStatus {
            detail: "Automatic update checks are unavailable for this installation source."
                .to_string(),
        };
    };
    cache::status(
        &cache::directory(),
        &channel_key,
        &current_version.to_string(),
    )
}

fn check_for_update_at(paths: &ControlPaths, force: bool, checked_at: SystemTime) -> StartupUpdate {
    if update_check_disabled() {
        return StartupUpdate::None;
    }
    let current_executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return StartupUpdate::Failed(structural(format!(
                "cannot locate the running FastCtx binary: {error}"
            )));
        }
    };
    let current_version = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(error) => {
            return StartupUpdate::Failed(structural(format!(
                "the embedded FastCtx version is invalid: {error}"
            )));
        }
    };
    let channel = match detect_install_channel(
        paths,
        &current_executable,
        &|name| std::env::var_os(name),
        option_env!("FASTCTX_DISTRIBUTION") == Some(GITHUB_RELEASE_DISTRIBUTION),
    ) {
        Ok(channel) => channel,
        Err(error) => return StartupUpdate::Failed(structural(error)),
    };
    let Some(channel_key) = channel_key(&channel) else {
        return StartupUpdate::None;
    };
    let directory = cache::directory();
    let version_text = current_version.to_string();
    resolve_with_cache(
        &channel,
        &channel_key,
        &version_text,
        &directory,
        force,
        checked_at,
        || probe_channel(paths, &channel, &current_version),
    )
}

fn resolve_with_cache(
    channel: &InstallChannel,
    channel_key: &str,
    current_version: &str,
    directory: &Path,
    force: bool,
    checked_at: SystemTime,
    probe: impl FnOnce() -> Result<CachedOutcome, CheckFailure>,
) -> StartupUpdate {
    if force {
        // A forced failure must not leave a still-fresh success suppressing the next startup retry.
        cache::invalidate_success(directory, channel_key);
    } else if let Some(cached) =
        cache::load_fresh_success(directory, channel_key, current_version, checked_at)
        && let Some(result) = startup_from_cached(channel, cached)
    {
        return result;
    }

    match probe() {
        Ok(cached) => {
            let Some(result) = startup_from_cached(channel, cached.clone()) else {
                return StartupUpdate::Failed(structural(
                    "the update check produced an outcome for the wrong installation channel",
                ));
            };
            if let Err(error) =
                cache::record_success(directory, channel_key, current_version, checked_at, &cached)
            {
                let mut message = format!(
                    "the update check succeeded, but its private cache could not be saved: {error}"
                );
                if let Err(status_error) = cache::record_failure(
                    directory,
                    channel_key,
                    current_version,
                    checked_at,
                    CheckFailureKind::Structural,
                    &message,
                ) {
                    message.push_str(&format!(
                        "; the private failure record could not be saved: {status_error}"
                    ));
                }
                return StartupUpdate::Failed(structural(message));
            }
            result
        }
        Err(failure) => {
            if let Err(cache_error) = cache::record_failure(
                directory,
                channel_key,
                current_version,
                checked_at,
                failure.kind,
                &failure.message,
            ) {
                return StartupUpdate::Failed(structural(format!(
                    "{}; the private failure record could not be saved: {cache_error}",
                    failure.message
                )));
            }
            StartupUpdate::Failed(failure)
        }
    }
}

fn probe_channel(
    paths: &ControlPaths,
    channel: &InstallChannel,
    current_version: &Version,
) -> Result<CachedOutcome, CheckFailure> {
    match channel {
        InstallChannel::Unsupported => Ok(CachedOutcome::Current),
        InstallChannel::GithubRelease => {
            let target_version = latest_github_version()?;
            if target_version <= *current_version {
                Ok(CachedOutcome::Current)
            } else {
                Ok(CachedOutcome::GithubAvailable {
                    target_version: target_version.to_string(),
                })
            }
        }
        InstallChannel::Npm(provenance) => {
            // These are independent publication surfaces. Concurrent queries keep the normal
            // no-update path bounded by the slower request rather than their combined timeouts.
            let (registry_result, release_result) = std::thread::scope(|scope| {
                let registry = scope.spawn(|| latest_npm_version(paths, provenance));
                let release = scope.spawn(latest_github_version);
                (
                    registry
                        .join()
                        .unwrap_or_else(|_| Err(transient("the npm update-check worker panicked"))),
                    release.join().unwrap_or_else(|_| {
                        Err(transient("the GitHub update-check worker panicked"))
                    }),
                )
            });
            npm_update_result(current_version, registry_result, release_result)
        }
    }
}

fn npm_update_result(
    current_version: &Version,
    registry_result: Result<Version, CheckFailure>,
    release_result: Result<Version, CheckFailure>,
) -> Result<CachedOutcome, CheckFailure> {
    let registry_version = registry_result?;
    if registry_version > *current_version {
        return Ok(CachedOutcome::NpmAvailable {
            target_version: registry_version.to_string(),
        });
    }
    match release_result {
        Ok(release_version) if release_version > *current_version => {
            Ok(CachedOutcome::NpmPending {
                release_version: release_version.to_string(),
                registry_version: registry_version.to_string(),
            })
        }
        Ok(_) => Ok(CachedOutcome::Current),
        Err(error) => Err(error),
    }
}

fn startup_from_cached(channel: &InstallChannel, cached: CachedOutcome) -> Option<StartupUpdate> {
    match (channel, cached) {
        (_, CachedOutcome::Current) => Some(StartupUpdate::None),
        (InstallChannel::GithubRelease, CachedOutcome::GithubAvailable { target_version }) => {
            github_update_plan(&target_version).map(StartupUpdate::Available)
        }
        (InstallChannel::Npm(provenance), CachedOutcome::NpmAvailable { target_version })
            if stable_version(&target_version).is_some() =>
        {
            Some(StartupUpdate::Available(UpdatePlan::Npm {
                provenance: provenance.clone(),
                target_version,
            }))
        }
        (
            InstallChannel::Npm(_),
            CachedOutcome::NpmPending {
                release_version,
                registry_version,
            },
        ) if stable_version(&release_version).is_some()
            && stable_version(&registry_version).is_some() =>
        {
            Some(StartupUpdate::NpmPending {
                release_version,
                registry_version,
            })
        }
        _ => None,
    }
}

fn github_update_plan(target_version: &str) -> Option<UpdatePlan> {
    stable_version(target_version)?;
    let archive_name = expected_release_archive_name()?.to_string();
    let tag = format!("v{target_version}");
    Some(UpdatePlan::GithubRelease {
        target_version: target_version.to_string(),
        archive_url: format!("{GITHUB_RELEASE_BASE}/{tag}/{archive_name}"),
        checksums_url: format!("{GITHUB_RELEASE_BASE}/{tag}/SHA256SUMS"),
        archive_name,
    })
}

fn stable_version(value: &str) -> Option<Version> {
    Version::parse(value)
        .ok()
        .filter(|version| version.pre.is_empty())
}

fn channel_key(channel: &InstallChannel) -> Option<String> {
    match channel {
        InstallChannel::Npm(provenance) => Some(format!("npm-{}", provenance.package)),
        InstallChannel::GithubRelease => Some("github-release".to_string()),
        InstallChannel::Unsupported => None,
    }
}

fn transient(message: impl Into<String>) -> CheckFailure {
    CheckFailure {
        kind: CheckFailureKind::Transient,
        message: message.into(),
    }
}

fn structural(message: impl Into<String>) -> CheckFailure {
    CheckFailure {
        kind: CheckFailureKind::Structural,
        message: message.into(),
    }
}

fn update_check_disabled() -> bool {
    std::env::var("FASTCTX_DISABLE_UPDATE_CHECK")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
}

fn detect_install_channel(
    paths: &ControlPaths,
    current_executable: &Path,
    get_env: &dyn Fn(&str) -> Option<OsString>,
    is_github_release_build: bool,
) -> Result<InstallChannel, String> {
    if get_env(NPM_MARKER_ENV).as_deref() == Some(OsStr::new("1")) {
        return npm_provenance(paths, get_env).map(InstallChannel::Npm);
    }
    // A regular path is not provenance: custom Cargo roots look identical to downloaded files.
    // Only the release workflow's embedded marker enables direct self-update (2026-07-17).
    if same_existing_path(current_executable, &paths.installed_binary)
        || looks_like_cargo_install(paths, current_executable, get_env)
        || looks_like_development_binary(current_executable)
        || !is_github_release_build
    {
        return Ok(InstallChannel::Unsupported);
    }
    let metadata = fs::symlink_metadata(current_executable).map_err(|error| {
        format!(
            "cannot inspect the running binary {}: {error}",
            crate::paths::display_path(current_executable)
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(
            "automatic GitHub Release updates require FastCtx to run from a regular file"
                .to_string(),
        );
    }
    if expected_release_archive_name().is_none() {
        return Ok(InstallChannel::Unsupported);
    }
    Ok(InstallChannel::GithubRelease)
}

fn npm_provenance(
    paths: &ControlPaths,
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<NpmProvenance, String> {
    let package = required_utf8_env(get_env, NPM_PACKAGE_ENV)?;
    if !matches!(package.as_str(), "fastctx" | "codex-fastctx") {
        return Err(format!(
            "the npm launcher reported unsupported package {package:?}"
        ));
    }
    let mode = match required_utf8_env(get_env, NPM_MODE_ENV)?.as_str() {
        "global" => NpmMode::Global,
        "exec" => NpmMode::Exec,
        value => {
            return Err(format!(
                "the npm launcher reported unsupported mode {value:?}"
            ));
        }
    };
    let node = required_regular_path(get_env, NODE_ENV)?;
    let npm_cli = required_regular_path(get_env, NPM_CLI_ENV)?;
    let launcher = required_regular_path(get_env, NPM_LAUNCHER_ENV)?;
    let launcher_pid = required_utf8_env(get_env, NPM_LAUNCHER_PID_ENV)?
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "the npm launcher reported an invalid process id".to_string())?;
    let handoff_file = PathBuf::from(
        get_env(NPM_HANDOFF_ENV)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("the npm launcher did not provide {NPM_HANDOFF_ENV}"))?,
    );
    let expected_handoff = paths
        .fastctx_dir
        .join("update")
        .join(format!("npm-launcher-{launcher_pid}.handoff"));
    if !handoff_file.is_absolute() || handoff_file != expected_handoff {
        return Err("the npm launcher reported an invalid update handoff path".to_string());
    }
    Ok(NpmProvenance {
        package,
        mode,
        node,
        npm_cli,
        launcher,
        launcher_pid,
        handoff_file,
    })
}

fn required_utf8_env(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    name: &str,
) -> Result<String, String> {
    get_env(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("the npm launcher did not provide {name}"))?
        .into_string()
        .map_err(|_| format!("the npm launcher provided non-UTF-8 {name}"))
}

fn required_regular_path(
    get_env: &dyn Fn(&str) -> Option<OsString>,
    name: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(
        get_env(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("the npm launcher did not provide {name}"))?,
    );
    if !path.is_absolute() {
        return Err(format!("the npm launcher provided a relative {name} path"));
    }
    let metadata = fs::metadata(&path).map_err(|error| {
        format!(
            "cannot inspect npm launcher path {}: {error}",
            crate::paths::display_path(&path)
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "npm launcher path {} is not a regular file",
            crate::paths::display_path(&path)
        ));
    }
    Ok(path)
}

fn looks_like_cargo_install(
    paths: &ControlPaths,
    current_executable: &Path,
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> bool {
    let cargo_home = get_env("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.home.join(".cargo"));
    current_executable
        .parent()
        .is_some_and(|parent| same_existing_path(parent, &cargo_home.join("bin")))
}

fn looks_like_development_binary(current_executable: &Path) -> bool {
    let components = current_executable
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    components
        .windows(2)
        .any(|pair| pair[0] == "target" && matches!(pair[1].as_str(), "debug" | "release"))
}

fn same_existing_path(left: &Path, right: &Path) -> bool {
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn latest_npm_version(
    paths: &ControlPaths,
    provenance: &NpmProvenance,
) -> Result<Version, CheckFailure> {
    let cache =
        create_private_temp_directory(&paths.fastctx_dir, "npm-check").map_err(transient)?;
    let result = run_npm_view(provenance, &cache);
    let _ = fs::remove_dir_all(&cache);
    result
}

fn run_npm_view(provenance: &NpmProvenance, cache: &Path) -> Result<Version, CheckFailure> {
    let mut command = crate::process_policy::noninteractive_command(&provenance.node);
    command
        .arg(&provenance.npm_cli)
        .args(npm_view_arguments(&provenance.package, cache))
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env_remove("NPM_CONFIG_OFFLINE")
        .env_remove("npm_config_offline")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = wait_with_output(
        command.spawn().map_err(|error| {
            transient(format!(
                "cannot start npm through {}: {error}",
                crate::paths::display_path(&provenance.node)
            ))
        })?,
        NPM_QUERY_TIMEOUT,
    )
    .map_err(transient)?;
    if !output.status.success() {
        let detail = one_line(&String::from_utf8_lossy(&output.stderr));
        return Err(transient(if detail.is_empty() {
            "npm could not read FastCtx's latest published version".to_string()
        } else {
            format!("npm could not read FastCtx's latest published version: {detail}")
        }));
    }
    parse_npm_latest(&output.stdout)
}

fn parse_npm_latest(bytes: &[u8]) -> Result<Version, CheckFailure> {
    let value: String = serde_json::from_slice(bytes).map_err(|error| {
        structural(format!("npm returned invalid latest-version JSON: {error}"))
    })?;
    let version = Version::parse(value.trim()).map_err(|error| {
        structural(format!(
            "npm returned invalid FastCtx version {value:?}: {error}"
        ))
    })?;
    if !version.pre.is_empty() {
        return Err(structural(format!(
            "npm latest points to prerelease FastCtx version {version}"
        )));
    }
    Ok(version)
}

fn npm_view_arguments(package: &str, cache: &Path) -> Vec<OsString> {
    vec![
        OsString::from("view"),
        OsString::from(package),
        OsString::from("dist-tags.latest"),
        OsString::from("--json"),
        OsString::from("--prefer-online"),
        OsString::from("--registry"),
        OsString::from(NPM_REGISTRY),
        OsString::from("--cache"),
        cache.as_os_str().to_os_string(),
        OsString::from("--fetch-retries"),
        OsString::from("1"),
        OsString::from("--fetch-timeout"),
        OsString::from("4000"),
        OsString::from("--loglevel"),
        OsString::from("error"),
    ]
}

struct ChildOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn wait_with_output(mut child: Child, timeout: Duration) -> Result<ChildOutput, String> {
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.by_ref()
                .take(MAX_NPM_OUTPUT_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_NPM_OUTPUT_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "npm stdout exceeded its safety limit",
                ));
            }
            Ok(bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.by_ref()
                .take(MAX_NPM_OUTPUT_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_NPM_OUTPUT_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "npm stderr exceeded its safety limit",
                ));
            }
            Ok(bytes)
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "npm update check exceeded {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("cannot wait for npm update check: {error}"));
            }
        }
    };
    let stdout = join_output_reader(stdout_reader, "output")?;
    let stderr = join_output_reader(stderr_reader, "diagnostics")?;
    Ok(ChildOutput {
        status: status?,
        stdout,
        stderr,
    })
}

fn join_output_reader(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    label: &str,
) -> Result<Vec<u8>, String> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    reader
        .join()
        .map_err(|_| format!("npm update {label} reader panicked"))?
        .map_err(|error| format!("cannot read npm update {label}: {error}"))
}

fn latest_github_version() -> Result<Version, CheckFailure> {
    let agent = ureq::AgentBuilder::new()
        .timeout(GITHUB_TIMEOUT)
        .timeout_connect(GITHUB_TIMEOUT)
        .timeout_read(GITHUB_TIMEOUT)
        .timeout_write(GITHUB_TIMEOUT)
        .redirects(0)
        .build();
    let response = match agent
        .get(GITHUB_LATEST_URL)
        .set("Accept", "text/html")
        .set("User-Agent", concat!("fastctx/", env!("CARGO_PKG_VERSION")))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error)) => {
            return Err(transient(format!(
                "GitHub update check could not connect: {error}"
            )));
        }
    };
    parse_latest_redirect(response.status(), response.header("Location"))
}

fn parse_latest_redirect(status: u16, location: Option<&str>) -> Result<Version, CheckFailure> {
    if !(300..400).contains(&status) {
        return Err(transient(format!(
            "GitHub latest-release endpoint returned HTTP {status} without the required redirect"
        )));
    }
    let location = location
        .ok_or_else(|| transient("GitHub latest-release redirect omitted its Location header"))?;
    let url = Url::parse(location)
        .map_err(|error| transient(format!("GitHub returned an invalid redirect URL: {error}")))?;
    const TAG_PREFIX: &str = "/yc-duan/fastctx/releases/tag/";
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with(TAG_PREFIX)
    {
        return Err(transient(
            "GitHub latest-release redirect pointed outside yc-duan/fastctx",
        ));
    }
    let tag = &url.path()[TAG_PREFIX.len()..];
    if tag.is_empty() || tag.contains('/') {
        return Err(transient(
            "GitHub latest-release redirect used an unexpected tag path",
        ));
    }
    let value = tag.strip_prefix('v').ok_or_else(|| {
        structural(format!(
            "GitHub latest release tag {tag:?} does not use the required v{{semver}} form"
        ))
    })?;
    let version = Version::parse(value).map_err(|error| {
        structural(format!(
            "GitHub latest release tag {tag:?} is not semantic versioning: {error}"
        ))
    })?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(structural(format!(
            "GitHub latest release tag {tag:?} is not a stable release tag"
        )));
    }
    Ok(version)
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

fn create_private_temp_directory(base: &Path, purpose: &str) -> Result<PathBuf, String> {
    let root = std::env::temp_dir();
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = root.join(format!(
            "fastctx-{purpose}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        match create_private_directory(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create isolated npm cache near {}: {error}",
                    crate::paths::display_path(base)
                ));
            }
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

fn one_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        InstallChannel, NODE_ENV, NPM_CLI_ENV, NPM_HANDOFF_ENV, NPM_LAUNCHER_ENV,
        NPM_LAUNCHER_PID_ENV, NPM_MARKER_ENV, NPM_MODE_ENV, NPM_PACKAGE_ENV,
        detect_install_channel, github_update_plan, npm_update_result, npm_view_arguments,
        parse_latest_redirect, parse_npm_latest, resolve_with_cache, transient,
    };
    use crate::control::paths::ControlPaths;
    use crate::update::cache::{self, CachedOutcome, SUCCESS_TTL};
    use crate::update::model::{CheckFailureKind, NpmMode, NpmProvenance, StartupUpdate};
    use semver::Version;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn npm_launcher_provenance_wins_over_executable_location() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let current = temp.path().join("target/debug/fastctx");
        let node = temp.path().join("node");
        let npm_cli = temp.path().join("npm-cli.js");
        let launcher = temp.path().join("launcher.js");
        let launcher_pid = 4242;
        let handoff = paths
            .fastctx_dir
            .join("update")
            .join(format!("npm-launcher-{launcher_pid}.handoff"));
        for path in [&node, &npm_cli, &launcher] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let environment = BTreeMap::from([
            (NPM_MARKER_ENV, OsString::from("1")),
            (NPM_PACKAGE_ENV, OsString::from("codex-fastctx")),
            (NPM_MODE_ENV, OsString::from("exec")),
            (NODE_ENV, node.into_os_string()),
            (NPM_CLI_ENV, npm_cli.into_os_string()),
            (NPM_LAUNCHER_ENV, launcher.into_os_string()),
            (
                NPM_LAUNCHER_PID_ENV,
                OsString::from(launcher_pid.to_string()),
            ),
            (NPM_HANDOFF_ENV, handoff.clone().into_os_string()),
        ]);

        let channel = detect_install_channel(
            &paths,
            &current,
            &|name| environment.get(name).cloned(),
            false,
        )
        .unwrap();
        let InstallChannel::Npm(provenance) = channel else {
            panic!("expected npm channel");
        };
        assert_eq!(provenance.package, "codex-fastctx");
        assert_eq!(provenance.mode, NpmMode::Exec);
        assert_eq!(provenance.launcher_pid, launcher_pid);
        assert_eq!(provenance.handoff_file, handoff);
    }

    #[test]
    fn npm_launcher_provenance_rejects_missing_relative_and_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let current = temp.path().join("target/debug/fastctx");
        let node = temp.path().join("node");
        let npm_cli = temp.path().join("npm-cli.js");
        let launcher = temp.path().join("launcher.js");
        let launcher_pid = 4242;
        let handoff = paths
            .fastctx_dir
            .join("update")
            .join(format!("npm-launcher-{launcher_pid}.handoff"));
        for path in [&node, &npm_cli, &launcher] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let valid = BTreeMap::from([
            (NPM_MARKER_ENV, OsString::from("1")),
            (NPM_PACKAGE_ENV, OsString::from("fastctx")),
            (NPM_MODE_ENV, OsString::from("global")),
            (NODE_ENV, node.into_os_string()),
            (NPM_CLI_ENV, npm_cli.into_os_string()),
            (NPM_LAUNCHER_ENV, launcher.into_os_string()),
            (
                NPM_LAUNCHER_PID_ENV,
                OsString::from(launcher_pid.to_string()),
            ),
            (NPM_HANDOFF_ENV, handoff.into_os_string()),
        ]);
        let detect = |environment: &BTreeMap<&str, OsString>| {
            detect_install_channel(
                &paths,
                &current,
                &|name| environment.get(name).cloned(),
                false,
            )
        };

        for missing in [
            NPM_PACKAGE_ENV,
            NPM_MODE_ENV,
            NODE_ENV,
            NPM_CLI_ENV,
            NPM_LAUNCHER_ENV,
            NPM_LAUNCHER_PID_ENV,
            NPM_HANDOFF_ENV,
        ] {
            let mut environment = valid.clone();
            environment.remove(missing);
            let error = detect(&environment).unwrap_err();
            assert!(
                error.contains("did not provide"),
                "missing {missing} produced {error:?}"
            );
        }

        let mut environment = valid.clone();
        environment.insert(NPM_PACKAGE_ENV, OsString::from("@other/fastctx"));
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("unsupported package")
        );

        let mut environment = valid.clone();
        environment.insert(NPM_MODE_ENV, OsString::from("portable"));
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("unsupported mode")
        );

        let mut environment = valid.clone();
        environment.insert(NODE_ENV, OsString::from("relative-node"));
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("relative FASTCTX_NODE_EXECUTABLE path")
        );

        let mut environment = valid.clone();
        environment.insert(NPM_LAUNCHER_PID_ENV, OsString::from("0"));
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("invalid process id")
        );

        let mut environment = valid;
        environment.insert(
            NPM_HANDOFF_ENV,
            temp.path().join("wrong.handoff").into_os_string(),
        );
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("invalid update handoff path")
        );
    }

    #[test]
    fn npm_check_bypasses_the_shared_cache_and_pins_the_public_registry() {
        let arguments = npm_view_arguments("fastctx", std::path::Path::new("/isolated/cache"))
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "view",
                "fastctx",
                "dist-tags.latest",
                "--json",
                "--prefer-online",
                "--registry",
                "https://registry.npmjs.org/",
                "--cache",
                "/isolated/cache",
                "--fetch-retries",
                "1",
                "--fetch-timeout",
                "4000",
                "--loglevel",
                "error",
            ]
        );
        assert!(!arguments.iter().any(|value| value == "clean"));
    }

    #[test]
    fn npm_latest_must_be_a_stable_semantic_version() {
        assert_eq!(
            parse_npm_latest(br#""0.2.0""#).unwrap(),
            Version::new(0, 2, 0)
        );
        assert!(
            parse_npm_latest(br#""0.2.0-beta.1""#)
                .unwrap_err()
                .message
                .contains("prerelease")
        );
    }

    #[test]
    fn npm_update_decision_distinguishes_cache_staleness_from_registry_propagation() {
        let current = Version::new(0, 1, 0);

        assert_eq!(
            npm_update_result(
                &current,
                Ok(Version::new(0, 2, 0)),
                Err(transient("GitHub unavailable")),
            )
            .unwrap(),
            CachedOutcome::NpmAvailable {
                target_version: "0.2.0".to_string()
            }
        );
        assert_eq!(
            npm_update_result(&current, Ok(current.clone()), Ok(Version::new(0, 2, 0)),).unwrap(),
            CachedOutcome::NpmPending {
                release_version: "0.2.0".to_string(),
                registry_version: "0.1.0".to_string(),
            }
        );
        assert_eq!(
            npm_update_result(
                &current,
                Err(transient("npm unavailable")),
                Ok(current.clone()),
            )
            .unwrap_err()
            .message,
            "npm unavailable"
        );
        assert_eq!(
            npm_update_result(
                &current,
                Ok(current.clone()),
                Err(transient("GitHub unavailable")),
            )
            .unwrap_err()
            .message,
            "GitHub unavailable"
        );
    }

    #[test]
    fn ttl_hit_is_a_zero_probe_oracle_and_force_bypasses_it() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let channel = InstallChannel::Npm(NpmProvenance {
            package: "fastctx".to_string(),
            mode: NpmMode::Exec,
            node: "node".into(),
            npm_cli: "npm-cli.js".into(),
            launcher: "launcher.js".into(),
            launcher_pid: 42,
            handoff_file: "handoff".into(),
        });
        cache::record_success(
            temp.path(),
            "npm-fastctx",
            "0.1.0",
            checked_at,
            &CachedOutcome::NpmAvailable {
                target_version: "0.2.0".to_string(),
            },
        )
        .unwrap();

        let result = resolve_with_cache(
            &channel,
            "npm-fastctx",
            "0.1.0",
            temp.path(),
            false,
            checked_at + SUCCESS_TTL,
            || panic!("a fresh cache hit must not call the network probe"),
        );
        assert!(matches!(result, StartupUpdate::Available(_)));

        let calls = std::cell::Cell::new(0);
        let result = resolve_with_cache(
            &channel,
            "npm-fastctx",
            "0.1.0",
            temp.path(),
            true,
            checked_at + Duration::from_secs(1),
            || {
                calls.set(calls.get() + 1);
                Ok(CachedOutcome::Current)
            },
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(result, StartupUpdate::None);
    }

    #[test]
    fn expired_cache_calls_the_injected_probe_once() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let channel = InstallChannel::Npm(NpmProvenance {
            package: "fastctx".to_string(),
            mode: NpmMode::Global,
            node: "node".into(),
            npm_cli: "npm-cli.js".into(),
            launcher: "launcher.js".into(),
            launcher_pid: 7,
            handoff_file: "handoff".into(),
        });
        cache::record_success(
            temp.path(),
            "npm-fastctx",
            "0.1.0",
            checked_at,
            &CachedOutcome::Current,
        )
        .unwrap();
        let calls = std::cell::Cell::new(0);
        let result = resolve_with_cache(
            &channel,
            "npm-fastctx",
            "0.1.0",
            temp.path(),
            false,
            checked_at + SUCCESS_TTL + Duration::from_secs(1),
            || {
                calls.set(calls.get() + 1);
                Ok(CachedOutcome::NpmAvailable {
                    target_version: "0.2.0".to_string(),
                })
            },
        );
        assert_eq!(calls.get(), 1);
        assert!(matches!(result, StartupUpdate::Available(_)));
    }

    #[test]
    fn a_partial_success_cache_write_becomes_a_retryable_structural_failure_record() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::create_dir(temp.path().join("cache-github-release.json")).unwrap();

        let result = resolve_with_cache(
            &InstallChannel::GithubRelease,
            "github-release",
            "0.1.0",
            temp.path(),
            false,
            checked_at,
            || Ok(CachedOutcome::Current),
        );

        let StartupUpdate::Failed(failure) = result else {
            panic!("an incomplete cache commit must be surfaced as a structural failure");
        };
        assert_eq!(failure.kind, CheckFailureKind::Structural);
        assert!(
            failure.message.contains("private cache could not be saved"),
            "{failure:?}"
        );
        assert_eq!(
            cache::load_fresh_success(
                temp.path(),
                "github-release",
                "0.1.0",
                checked_at + Duration::from_secs(1)
            ),
            None
        );
        let detail = cache::status(temp.path(), "github-release", "0.1.0").detail;
        assert!(
            detail.contains("structural failure: the update check succeeded"),
            "{detail}"
        );
    }

    #[test]
    fn only_marked_direct_release_files_use_the_github_channel() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        std::fs::write(&paths.installed_binary, b"applied copy").unwrap();
        assert!(matches!(
            detect_install_channel(&paths, &paths.installed_binary, &|_| None, true).unwrap(),
            InstallChannel::Unsupported
        ));

        let cargo_binary = temp.path().join(".cargo").join("bin").join("fastctx");
        std::fs::create_dir_all(cargo_binary.parent().unwrap()).unwrap();
        std::fs::write(&cargo_binary, b"cargo install").unwrap();
        assert!(matches!(
            detect_install_channel(&paths, &cargo_binary, &|_| None, true).unwrap(),
            InstallChannel::Unsupported
        ));

        let direct = temp.path().join("downloads").join(if cfg!(windows) {
            "fastctx.exe"
        } else {
            "fastctx"
        });
        std::fs::create_dir_all(direct.parent().unwrap()).unwrap();
        std::fs::write(&direct, b"release download").unwrap();
        assert!(matches!(
            detect_install_channel(&paths, &direct, &|_| None, false).unwrap(),
            InstallChannel::Unsupported
        ));
        let channel = detect_install_channel(&paths, &direct, &|_| None, true).unwrap();
        if super::expected_release_archive_name().is_some() {
            assert!(matches!(channel, InstallChannel::GithubRelease));
        } else {
            assert!(matches!(channel, InstallChannel::Unsupported));
        }
    }

    #[test]
    fn github_latest_redirect_is_not_followed_and_accepts_only_the_exact_stable_tag_shape() {
        assert_eq!(
            parse_latest_redirect(
                302,
                Some("https://github.com/yc-duan/fastctx/releases/tag/v0.2.0")
            )
            .unwrap(),
            Version::new(0, 2, 0)
        );
        assert_eq!(
            parse_latest_redirect(
                301,
                Some("https://github.com/yc-duan/fastctx/releases/tag/v1.0.0")
            )
            .unwrap(),
            Version::new(1, 0, 0)
        );
        for (status, location) in [
            (
                200,
                Some("https://github.com/yc-duan/fastctx/releases/tag/v0.2.0"),
            ),
            (
                302,
                Some("https://example.com/yc-duan/fastctx/releases/tag/v0.2.0"),
            ),
            (
                302,
                Some("https://github.com/other/fastctx/releases/tag/v0.2.0"),
            ),
            (302, None),
        ] {
            let failure = parse_latest_redirect(status, location).unwrap_err();
            assert_eq!(failure.kind, CheckFailureKind::Transient, "{failure:?}");
        }
        for location in [
            "https://github.com/yc-duan/fastctx/releases/tag/v0.2.0-beta.1",
            "https://github.com/yc-duan/fastctx/releases/tag/0.2.0",
            "https://github.com/yc-duan/fastctx/releases/tag/not-a-version",
        ] {
            let failure = parse_latest_redirect(302, Some(location)).unwrap_err();
            assert_eq!(failure.kind, CheckFailureKind::Structural, "{failure:?}");
        }
    }

    #[test]
    fn github_plan_pins_archive_and_aggregate_checksum_to_the_validated_tag() {
        let Some(plan) = github_update_plan("0.2.0") else {
            return;
        };
        let crate::update::model::UpdatePlan::GithubRelease {
            target_version,
            archive_name,
            archive_url,
            checksums_url,
        } = plan
        else {
            panic!("expected GitHub plan");
        };
        assert_eq!(target_version, "0.2.0");
        assert!(archive_url.ends_with(&format!("/v0.2.0/{archive_name}")));
        assert_eq!(
            checksums_url,
            "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/SHA256SUMS"
        );
        assert!(archive_name.ends_with(".zip") || archive_name.ends_with(".tar.gz"));
        assert!(github_update_plan("0.2.0-beta.1").is_none());
    }
}
