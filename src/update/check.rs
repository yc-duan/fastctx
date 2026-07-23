//! Installation-source detection, cached discovery, and classified update failures.

use super::cache::{self, CachedOutcome, CheckStatus};
use super::model::{
    CheckFailure, CheckFailureKind, NPMMIRROR_REGISTRY, NpmDiscovery, NpmDriver, NpmMode,
    NpmProvenance, NpmRegistryProbe, NpmVersionAuthority, OFFICIAL_NPM_REGISTRY, StartupUpdate,
    UpdatePlan,
};
use crate::control::paths::ControlPaths;
use crate::control::settings::{self, UpdateSource};
use semver::Version;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
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
const GITHUB_RELEASE_DISTRIBUTION: &str = "github-release";
const NPM_QUERY_TIMEOUT: Duration = Duration::from_secs(8);
const GITHUB_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_NPM_OUTPUT_BYTES: u64 = 1024 * 1024;
const NPM_MARKER_ENV: &str = "FASTCTX_NPM_LAUNCHER_VERSION";
const NPM_PACKAGE_ENV: &str = "FASTCTX_NPM_PACKAGE";
const NPM_MODE_ENV: &str = "FASTCTX_NPM_MODE";
const NODE_ENV: &str = "FASTCTX_NODE_EXECUTABLE";
const NPM_DRIVER_ENV: &str = "FASTCTX_NPM_DRIVER";
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NpmReceiptVersion {
    V1,
    V2,
}

#[derive(Clone, Debug)]
struct NpmCheckContext {
    source_policy: UpdateSource,
    configured_registry: Option<String>,
    configured_registry_failure: Option<CheckFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryCandidate {
    source_name: String,
    registry: String,
    selectable: bool,
}

#[derive(Clone, Debug)]
struct RegistryLatest {
    candidate: RegistryCandidate,
    result: Result<Version, CheckFailure>,
}

struct CacheResolution<'a> {
    channel: &'a InstallChannel,
    channel_key: &'a str,
    current_version: &'a str,
    directory: &'a Path,
    force: bool,
    checked_at: SystemTime,
    npm_context: Option<&'a NpmCheckContext>,
}

trait NpmProbeBackend: Sync {
    fn latest_github_version(&self) -> Result<Version, CheckFailure>;

    fn latest_npm_version(&self, registry: &str, package: &str) -> Result<Version, CheckFailure>;

    fn exact_npm_version_exists(
        &self,
        registry: &str,
        package: &str,
        target_version: &str,
    ) -> Result<bool, CheckFailure>;
}

struct LiveNpmProbeBackend<'a> {
    paths: &'a ControlPaths,
    provenance: &'a NpmProvenance,
}

impl NpmProbeBackend for LiveNpmProbeBackend<'_> {
    fn latest_github_version(&self) -> Result<Version, CheckFailure> {
        latest_github_version()
    }

    fn latest_npm_version(&self, registry: &str, package: &str) -> Result<Version, CheckFailure> {
        latest_npm_version(self.paths, self.provenance, registry, package)
    }

    fn exact_npm_version_exists(
        &self,
        registry: &str,
        package: &str,
        target_version: &str,
    ) -> Result<bool, CheckFailure> {
        exact_npm_version_exists(
            self.paths,
            self.provenance,
            registry,
            package,
            target_version,
        )
    }
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
    let update_status = match settings::update_settings_status(paths) {
        Ok(status) => status,
        Err(error) => {
            return CheckStatus {
                detail: format!("Cannot read update settings: {error}"),
            };
        }
    };
    let mut status_prefix = format!("source={}", update_status.source.as_str());
    if update_status.source_fell_back {
        status_prefix.push_str(" (invalid stored value fell back to auto)");
    }
    if update_check_disabled() {
        status_prefix.push_str("; automatic checks disabled by FASTCTX_DISABLE_UPDATE_CHECK");
    }
    if !update_status.auto_check {
        status_prefix.push_str(
            "; automatic checks disabled by update.auto_check (manual checks remain available)",
        );
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
    let npm_context = match &channel {
        InstallChannel::Npm(provenance) => {
            Some(prepare_npm_context(provenance, update_status.source))
        }
        _ => None,
    };
    let Some(channel_key) = channel_key(&channel, npm_context.as_ref()) else {
        return CheckStatus {
            detail: format!(
                "{status_prefix}; automatic update checks are unavailable for this installation source."
            ),
        };
    };
    let cached = cache::status(
        &cache::directory(),
        &channel_key,
        &current_version.to_string(),
    );
    CheckStatus {
        detail: format!("{status_prefix}; {}", cached.detail),
    }
}

fn check_for_update_at(paths: &ControlPaths, force: bool, checked_at: SystemTime) -> StartupUpdate {
    let settings = match settings::load(paths) {
        Ok(settings) => settings,
        Err(error) => {
            return StartupUpdate::Failed(structural(format!(
                "cannot read update settings: {error}"
            )));
        }
    };
    run_update_check_if_enabled(
        force,
        update_check_disabled(),
        settings.update.auto_check,
        || check_for_update_enabled(paths, force, checked_at, settings.update.source),
    )
    .unwrap_or(StartupUpdate::None)
}

fn check_for_update_enabled(
    paths: &ControlPaths,
    force: bool,
    checked_at: SystemTime,
    update_source: UpdateSource,
) -> StartupUpdate {
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
    let npm_context = match &channel {
        InstallChannel::Npm(provenance) => Some(prepare_npm_context(provenance, update_source)),
        _ => None,
    };
    let Some(channel_key) = channel_key(&channel, npm_context.as_ref()) else {
        return StartupUpdate::None;
    };
    let directory = cache::directory();
    let version_text = current_version.to_string();
    resolve_with_cache(
        CacheResolution {
            channel: &channel,
            channel_key: &channel_key,
            current_version: &version_text,
            directory: &directory,
            force,
            checked_at,
            npm_context: npm_context.as_ref(),
        },
        || probe_channel(paths, &channel, &current_version, npm_context.as_ref()),
    )
}

fn resolve_with_cache(
    request: CacheResolution<'_>,
    probe: impl FnOnce() -> Result<CachedOutcome, CheckFailure>,
) -> StartupUpdate {
    let CacheResolution {
        channel,
        channel_key,
        current_version,
        directory,
        force,
        checked_at,
        npm_context,
    } = request;
    if force {
        // A forced failure must not leave a still-fresh success suppressing the next startup retry.
        cache::invalidate_success(directory, channel_key);
    } else if let Some(cached) =
        cache::load_fresh_success(directory, channel_key, current_version, checked_at)
        && let Some(result) = startup_from_cached(channel, cached, npm_context)
    {
        return result;
    }

    match probe() {
        Ok(cached) => {
            let Some(result) = startup_from_cached(channel, cached.clone(), npm_context) else {
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
    npm_context: Option<&NpmCheckContext>,
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
        InstallChannel::Npm(provenance) => probe_npm_channel(
            paths,
            provenance,
            current_version,
            npm_context.expect("npm channels always have source context"),
        ),
    }
}

fn probe_npm_channel(
    paths: &ControlPaths,
    provenance: &NpmProvenance,
    current_version: &Version,
    context: &NpmCheckContext,
) -> Result<CachedOutcome, CheckFailure> {
    let backend = LiveNpmProbeBackend { paths, provenance };
    probe_npm_channel_with_backend(provenance, current_version, context, &backend)
}

fn probe_npm_channel_with_backend(
    provenance: &NpmProvenance,
    current_version: &Version,
    context: &NpmCheckContext,
    backend: &(impl NpmProbeBackend + ?Sized),
) -> Result<CachedOutcome, CheckFailure> {
    let candidates = registry_candidates(context)?;
    let mut registries = candidates.clone();
    if !registries
        .iter()
        .any(|candidate| candidate.registry == OFFICIAL_NPM_REGISTRY)
    {
        registries.push(RegistryCandidate {
            source_name: "official npm (version authority)".to_string(),
            registry: OFFICIAL_NPM_REGISTRY.to_string(),
            selectable: false,
        });
    }

    let (latest, github_result) = std::thread::scope(|scope| {
        let github = scope.spawn(|| backend.latest_github_version());
        let handles = registries
            .into_iter()
            .map(|candidate| {
                scope.spawn(move || RegistryLatest {
                    result: backend.latest_npm_version(&candidate.registry, "fastctx"),
                    candidate,
                })
            })
            .collect::<Vec<_>>();
        let latest = handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| RegistryLatest {
                    candidate: RegistryCandidate {
                        source_name: "unknown npm source".to_string(),
                        registry: String::new(),
                        selectable: false,
                    },
                    result: Err(transient("an npm source-check worker panicked")),
                })
            })
            .collect::<Vec<_>>();
        let github_result = github
            .join()
            .unwrap_or_else(|_| Err(transient("the GitHub update-check worker panicked")));
        (latest, github_result)
    });

    let official_result = latest
        .iter()
        .find(|probe| probe.candidate.registry == OFFICIAL_NPM_REGISTRY)
        .map(|probe| probe.result.clone())
        .unwrap_or_else(|| Err(transient("the official npm registry was not probed")));
    let (target_version, authority) =
        authoritative_npm_target(&github_result, &official_result, &latest)?;
    let github_version = github_result.as_ref().ok().map(ToString::to_string);
    let official_version = official_result.as_ref().ok().map(ToString::to_string);
    let platform_package = platform_npm_package()
        .ok_or_else(|| structural("this npm platform has no published FastCtx package"))?;
    let probes = probe_registry_readiness(
        backend,
        provenance,
        latest,
        &target_version,
        platform_package,
    );
    Ok(build_npm_outcome(
        context,
        current_version,
        &candidates,
        target_version,
        authority,
        github_version,
        official_version,
        platform_package,
        probes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_npm_outcome(
    context: &NpmCheckContext,
    current_version: &Version,
    candidates: &[RegistryCandidate],
    target_version: Version,
    authority: NpmVersionAuthority,
    github_version: Option<String>,
    official_version: Option<String>,
    platform_package: &str,
    probes: Vec<NpmRegistryProbe>,
) -> CachedOutcome {
    let selected = select_ready_candidate(candidates, &probes);
    let (selected_registry, selected_source) = selected
        .map(|(registry, source)| (Some(registry), Some(source)))
        .unwrap_or((None, None));
    let selection_reason = match (&selected_source, context.source_policy) {
        (Some(source), UpdateSource::Auto) => {
            format!("auto selected the first reachable complete source: {source}")
        }
        (Some(source), _) => format!("the configured source policy selected {source}"),
        (None, UpdateSource::Auto) => {
            "no auto candidate has both the main and platform packages yet".to_string()
        }
        (None, _)
            if candidates.iter().any(|candidate| {
                probes
                    .iter()
                    .any(|probe| probe.registry == candidate.registry && probe.reachable)
            }) =>
        {
            "the configured source is reachable but not complete yet".to_string()
        }
        (None, _) => "the configured source could not be reached".to_string(),
    };
    let discovery = NpmDiscovery {
        source_policy: context.source_policy.as_str().to_string(),
        configured_registry: context.configured_registry.clone(),
        target_version: target_version.to_string(),
        authority,
        github_version,
        official_version,
        platform_package: platform_package.to_string(),
        probes,
        selected_registry,
        selected_source,
        selection_reason,
    };

    if target_version <= *current_version {
        CachedOutcome::NpmCurrent { discovery }
    } else if discovery.selected_registry.is_some() {
        CachedOutcome::NpmAvailable { discovery }
    } else {
        CachedOutcome::NpmPending { discovery }
    }
}

fn select_ready_candidate(
    candidates: &[RegistryCandidate],
    probes: &[NpmRegistryProbe],
) -> Option<(String, String)> {
    candidates.iter().find_map(|candidate| {
        probes
            .iter()
            .find(|probe| probe.registry == candidate.registry && probe.is_ready())
            .map(|probe| (probe.registry.clone(), probe.source_name.clone()))
    })
}

fn registry_candidates(context: &NpmCheckContext) -> Result<Vec<RegistryCandidate>, CheckFailure> {
    let configured = || {
        context
            .configured_registry
            .clone()
            .map(|registry| RegistryCandidate {
                source_name: "npm config".to_string(),
                registry,
                selectable: true,
            })
    };
    let official = || RegistryCandidate {
        source_name: "official npm".to_string(),
        registry: OFFICIAL_NPM_REGISTRY.to_string(),
        selectable: true,
    };
    let mirror = || RegistryCandidate {
        source_name: "npmmirror".to_string(),
        registry: NPMMIRROR_REGISTRY.to_string(),
        selectable: true,
    };
    let raw = match context.source_policy {
        UpdateSource::Auto => configured()
            .into_iter()
            .chain([official(), mirror()])
            .collect::<Vec<_>>(),
        UpdateSource::NpmConfig => vec![configured().ok_or_else(|| {
            context
                .configured_registry_failure
                .clone()
                .unwrap_or_else(|| {
                    transient("npm config get registry did not return a usable registry")
                })
        })?],
        UpdateSource::Official => vec![official()],
        UpdateSource::Npmmirror => vec![mirror()],
    };
    let mut seen = BTreeSet::new();
    Ok(raw
        .into_iter()
        .filter(|candidate| seen.insert(candidate.registry.clone()))
        .collect())
}

fn authoritative_npm_target(
    github: &Result<Version, CheckFailure>,
    official: &Result<Version, CheckFailure>,
    sources: &[RegistryLatest],
) -> Result<(Version, NpmVersionAuthority), CheckFailure> {
    let official_versions = [github.as_ref().ok(), official.as_ref().ok()]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if let Some(version) = official_versions.into_iter().max() {
        return Ok((version, NpmVersionAuthority::Official));
    }
    if let Some(version) = sources
        .iter()
        .filter(|source| source.candidate.selectable)
        .filter_map(|source| source.result.as_ref().ok())
        .cloned()
        .max()
    {
        return Ok((version, NpmVersionAuthority::MirrorFallback));
    }
    let failures = std::iter::once(github.as_ref().err())
        .chain(std::iter::once(official.as_ref().err()))
        .chain(sources.iter().map(|source| source.result.as_ref().err()))
        .flatten()
        .collect::<Vec<_>>();
    let kind = if failures
        .iter()
        .any(|failure| failure.kind == CheckFailureKind::Structural)
    {
        CheckFailureKind::Structural
    } else {
        CheckFailureKind::Transient
    };
    let detail = failures
        .iter()
        .map(|failure| failure.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    Err(CheckFailure {
        kind,
        message: format!("no npm version authority was reachable: {detail}"),
    })
}

fn probe_registry_readiness(
    backend: &(impl NpmProbeBackend + ?Sized),
    provenance: &NpmProvenance,
    latest: Vec<RegistryLatest>,
    target_version: &Version,
    platform_package: &str,
) -> Vec<NpmRegistryProbe> {
    std::thread::scope(|scope| {
        latest
            .into_iter()
            .map(|latest| {
                scope.spawn(move || {
                    registry_readiness(
                        backend,
                        provenance,
                        latest,
                        target_version,
                        platform_package,
                    )
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| NpmRegistryProbe {
                    source_name: "unknown npm source".to_string(),
                    registry: String::new(),
                    reachable: false,
                    latest_version: None,
                    main_package_ready: false,
                    platform_package_ready: false,
                    error: Some("an npm preflight worker panicked".to_string()),
                    error_kind: Some(CheckFailureKind::Transient),
                })
            })
            .collect()
    })
}

fn registry_readiness(
    backend: &(impl NpmProbeBackend + ?Sized),
    provenance: &NpmProvenance,
    latest: RegistryLatest,
    target_version: &Version,
    platform_package: &str,
) -> NpmRegistryProbe {
    let RegistryLatest { candidate, result } = latest;
    let latest_version = match result {
        Ok(version) => version,
        Err(failure) => {
            return NpmRegistryProbe {
                source_name: candidate.source_name,
                registry: candidate.registry,
                reachable: false,
                latest_version: None,
                main_package_ready: false,
                platform_package_ready: false,
                error: Some(failure.message),
                error_kind: Some(failure.kind),
            };
        }
    };
    let target = target_version.to_string();
    let mut packages = vec!["fastctx".to_string()];
    if provenance.package != "fastctx" {
        packages.push(provenance.package.clone());
    }
    packages.push(platform_package.to_string());
    let results = std::thread::scope(|scope| {
        packages
            .iter()
            .map(|package| {
                scope.spawn(|| {
                    backend.exact_npm_version_exists(&candidate.registry, package, &target)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(transient("an npm package preflight worker panicked")))
            })
            .collect::<Vec<_>>()
    });
    let main_count = if provenance.package == "fastctx" {
        1
    } else {
        2
    };
    let main_package_ready = results
        .iter()
        .take(main_count)
        .all(|result| matches!(result, Ok(true)));
    let platform_package_ready = matches!(results.last(), Some(Ok(true)));
    let failure = results.iter().find_map(|result| result.as_ref().err());
    NpmRegistryProbe {
        source_name: candidate.source_name,
        registry: candidate.registry,
        reachable: true,
        latest_version: Some(latest_version.to_string()),
        main_package_ready,
        platform_package_ready,
        error: failure.map(|failure| failure.message.clone()),
        error_kind: failure.map(|failure| failure.kind),
    }
}

fn startup_from_cached(
    channel: &InstallChannel,
    cached: CachedOutcome,
    npm_context: Option<&NpmCheckContext>,
) -> Option<StartupUpdate> {
    match (channel, cached) {
        (_, CachedOutcome::Current) => Some(StartupUpdate::None),
        (InstallChannel::GithubRelease, CachedOutcome::GithubAvailable { target_version }) => {
            github_update_plan(&target_version)
                .map(Box::new)
                .map(StartupUpdate::Available)
        }
        (InstallChannel::Npm(_), CachedOutcome::NpmCurrent { discovery })
            if discovery_matches_context(&discovery, npm_context?) =>
        {
            Some(StartupUpdate::NpmCurrent {
                discovery: Box::new(discovery),
            })
        }
        (InstallChannel::Npm(provenance), CachedOutcome::NpmAvailable { discovery })
            if discovery_matches_context(&discovery, npm_context?)
                && stable_version(&discovery.target_version).is_some()
                && discovery.selected_registry.is_some()
                && discovery.selected_source.is_some() =>
        {
            let target_version = discovery.target_version.clone();
            Some(StartupUpdate::Available(Box::new(UpdatePlan::Npm {
                provenance: provenance.clone(),
                target_version,
                registry: discovery.selected_registry.clone()?,
                source_name: discovery.selected_source.clone()?,
                discovery: Box::new(discovery),
            })))
        }
        (InstallChannel::Npm(_), CachedOutcome::NpmPending { discovery })
            if discovery_matches_context(&discovery, npm_context?)
                && stable_version(&discovery.target_version).is_some() =>
        {
            Some(StartupUpdate::NpmPending {
                target_version: discovery.target_version.clone(),
                discovery: Box::new(discovery),
            })
        }
        _ => None,
    }
}

fn discovery_matches_context(discovery: &NpmDiscovery, context: &NpmCheckContext) -> bool {
    discovery.source_policy == context.source_policy.as_str()
        && discovery.configured_registry == context.configured_registry
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

fn channel_key(channel: &InstallChannel, npm_context: Option<&NpmCheckContext>) -> Option<String> {
    match channel {
        InstallChannel::Npm(provenance) => {
            let context = npm_context?;
            let registry = context
                .configured_registry
                .as_deref()
                .unwrap_or("unavailable");
            let digest = Sha256::digest(registry.as_bytes());
            let fingerprint = digest[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            Some(format!(
                "npm-{}-{}-{fingerprint}",
                provenance.package,
                context.source_policy.as_str()
            ))
        }
        InstallChannel::GithubRelease => Some("github-release".to_string()),
        InstallChannel::Unsupported => None,
    }
}

fn prepare_npm_context(provenance: &NpmProvenance, source_policy: UpdateSource) -> NpmCheckContext {
    if !matches!(source_policy, UpdateSource::Auto | UpdateSource::NpmConfig) {
        return NpmCheckContext {
            source_policy,
            configured_registry: None,
            configured_registry_failure: None,
        };
    }
    match effective_npm_registry(provenance) {
        Ok(registry) => NpmCheckContext {
            source_policy,
            configured_registry: Some(registry),
            configured_registry_failure: None,
        },
        Err(failure) => NpmCheckContext {
            source_policy,
            configured_registry: None,
            configured_registry_failure: Some(failure),
        },
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

fn automatic_check_disabled(force: bool, environment_disabled: bool, auto_check: bool) -> bool {
    !force && (environment_disabled || !auto_check)
}

fn run_update_check_if_enabled<T>(
    force: bool,
    environment_disabled: bool,
    auto_check: bool,
    check: impl FnOnce() -> T,
) -> Option<T> {
    (!automatic_check_disabled(force, environment_disabled, auto_check)).then(check)
}

fn detect_install_channel(
    paths: &ControlPaths,
    current_executable: &Path,
    get_env: &dyn Fn(&str) -> Option<OsString>,
    is_github_release_build: bool,
) -> Result<InstallChannel, String> {
    if let Some(receipt_version) = npm_receipt_version(get_env)? {
        return npm_provenance(paths, get_env, receipt_version).map(InstallChannel::Npm);
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

fn npm_receipt_version(
    get_env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<NpmReceiptVersion>, String> {
    let Some(value) = get_env(NPM_MARKER_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| "the npm launcher reported a non-UTF-8 receipt version".to_string())?;
    match value.as_str() {
        "1" => Ok(Some(NpmReceiptVersion::V1)),
        "2" => Ok(Some(NpmReceiptVersion::V2)),
        _ => Err(format!(
            "the npm launcher reported unsupported receipt version {value:?}; reinstall FastCtx with `npm install --global fastctx --registry=https://registry.npmjs.org/`"
        )),
    }
}

fn npm_provenance(
    paths: &ControlPaths,
    get_env: &dyn Fn(&str) -> Option<OsString>,
    receipt_version: NpmReceiptVersion,
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
    let driver = match receipt_version {
        NpmReceiptVersion::V1 => NpmDriver::NodeScript,
        NpmReceiptVersion::V2 => match required_utf8_env(get_env, NPM_DRIVER_ENV)?.as_str() {
            "node-script" => NpmDriver::NodeScript,
            "executable" => NpmDriver::Executable,
            "unavailable" => {
                return Err(format!(
                    "FastCtx was launched from npm, but no usable npm command was found for Node.js at {}. Run `npm --version` in this terminal; if it fails, repair Node.js/npm. Then reinstall FastCtx with `npm install --global {package} --registry=https://registry.npmjs.org/`",
                    crate::paths::display_path(&node)
                ));
            }
            value => {
                return Err(format!(
                    "the npm launcher reported unsupported npm driver {value:?}"
                ));
            }
        },
    };
    let npm_cli = required_regular_path(get_env, NPM_CLI_ENV)?;
    validate_npm_driver_path(driver, &npm_cli)?;
    Ok(NpmProvenance {
        package,
        mode,
        node,
        driver,
        npm_cli,
        launcher,
        launcher_pid,
        handoff_file,
    })
}

fn validate_npm_driver_path(driver: NpmDriver, path: &Path) -> Result<(), String> {
    if driver == NpmDriver::NodeScript
        && !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "npm-cli.js" | "npm-cli.cjs" | "npm-cli.mjs"
                )
            })
    {
        return Err("the npm launcher reported a non-npm Node.js entry point".to_string());
    }
    if driver == NpmDriver::Executable
        && cfg!(windows)
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension.to_ascii_lowercase().as_str(), "cmd" | "bat")
            })
    {
        return Err("the npm launcher reported a shell script as an executable driver".to_string());
    }
    Ok(())
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

fn effective_npm_registry(provenance: &NpmProvenance) -> Result<String, CheckFailure> {
    let mut command = super::npm_invocation::noninteractive_command(provenance);
    command
        .args(["config", "get", "registry"])
        .env("NO_UPDATE_NOTIFIER", "1")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = wait_with_output(
        command.spawn().map_err(|error| {
            transient(format!(
                "cannot start npm through {}: {error}",
                crate::paths::display_path(super::npm_invocation::program(provenance))
            ))
        })?,
        NPM_QUERY_TIMEOUT,
    )
    .map_err(transient)?;
    if !output.status.success() {
        let detail = one_line(&String::from_utf8_lossy(&output.stderr));
        return Err(transient(if detail.is_empty() {
            "npm config get registry failed".to_string()
        } else {
            format!("npm config get registry failed: {detail}")
        }));
    }
    normalize_registry_url(one_line(&String::from_utf8_lossy(&output.stdout)))
}

fn normalize_registry_url(value: String) -> Result<String, CheckFailure> {
    if value.is_empty() || matches!(value.as_str(), "null" | "undefined") {
        return Err(transient(
            "npm config get registry returned no usable registry",
        ));
    }
    let mut url = Url::parse(&value)
        .map_err(|error| transient(format!("npm returned an invalid registry URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(transient(
            "npm returned a registry URL with an unsupported or unsafe shape",
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url.to_string())
}

fn latest_npm_version(
    paths: &ControlPaths,
    provenance: &NpmProvenance,
    registry: &str,
    package: &str,
) -> Result<Version, CheckFailure> {
    let cache =
        create_private_temp_directory(&paths.fastctx_dir, "npm-check").map_err(transient)?;
    let result = run_npm_view(provenance, package, registry, &cache).and_then(|version| {
        version.ok_or_else(|| {
            transient(format!(
                "npm registry {registry} does not expose {package}'s latest version"
            ))
        })
    });
    let _ = fs::remove_dir_all(&cache);
    result
}

fn exact_npm_version_exists(
    paths: &ControlPaths,
    provenance: &NpmProvenance,
    registry: &str,
    package: &str,
    target_version: &str,
) -> Result<bool, CheckFailure> {
    let cache =
        create_private_temp_directory(&paths.fastctx_dir, "npm-preflight").map_err(transient)?;
    let package_spec = format!("{package}@{target_version}");
    let result = run_npm_view(provenance, &package_spec, registry, &cache).and_then(|version| {
        let Some(version) = version else {
            return Ok(false);
        };
        if version.to_string() != target_version {
            return Err(structural(format!(
                "npm registry {registry} returned v{version} for exact request {package_spec}"
            )));
        }
        Ok(true)
    });
    let _ = fs::remove_dir_all(&cache);
    result
}

fn run_npm_view(
    provenance: &NpmProvenance,
    package_spec: &str,
    registry: &str,
    cache: &Path,
) -> Result<Option<Version>, CheckFailure> {
    let mut command = super::npm_invocation::noninteractive_command(provenance);
    command
        .args(npm_view_arguments(package_spec, registry, cache))
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
                crate::paths::display_path(super::npm_invocation::program(provenance))
            ))
        })?,
        NPM_QUERY_TIMEOUT,
    )
    .map_err(transient)?;
    if !output.status.success() {
        let detail = one_line(&String::from_utf8_lossy(&output.stderr));
        if detail.contains("E404") || detail.contains("404") {
            return Ok(None);
        }
        return Err(transient(if detail.is_empty() {
            format!("npm could not query {package_spec} from {registry}")
        } else {
            format!("npm could not query {package_spec} from {registry}: {detail}")
        }));
    }
    parse_npm_latest(&output.stdout).map(Some)
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

fn npm_view_arguments(package_spec: &str, registry: &str, cache: &Path) -> Vec<OsString> {
    vec![
        OsString::from("view"),
        OsString::from(package_spec),
        OsString::from("version"),
        OsString::from("--json"),
        OsString::from("--prefer-online"),
        OsString::from("--registry"),
        OsString::from(registry),
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

fn platform_npm_package() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("@fastctx/win32-x64"),
        ("linux", "x86_64") => Some("@fastctx/linux-x64"),
        ("macos", "x86_64") => Some("@fastctx/darwin-x64"),
        ("macos", "aarch64") => Some("@fastctx/darwin-arm64"),
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
        CacheResolution, InstallChannel, NODE_ENV, NPM_CLI_ENV, NPM_DRIVER_ENV, NPM_HANDOFF_ENV,
        NPM_LAUNCHER_ENV, NPM_LAUNCHER_PID_ENV, NPM_MARKER_ENV, NPM_MODE_ENV, NPM_PACKAGE_ENV,
        NpmCheckContext, NpmProbeBackend, RegistryCandidate, RegistryLatest,
        authoritative_npm_target, automatic_check_disabled, build_npm_outcome,
        detect_install_channel, github_update_plan, normalize_registry_url, npm_view_arguments,
        parse_latest_redirect, parse_npm_latest, probe_npm_channel_with_backend,
        registry_candidates, resolve_with_cache, run_update_check_if_enabled,
        select_ready_candidate, transient,
    };
    use crate::control::paths::ControlPaths;
    use crate::control::settings::UpdateSource;
    use crate::update::cache::{self, CachedOutcome, SUCCESS_TTL};
    use crate::update::model::{
        CheckFailure, CheckFailureKind, NPMMIRROR_REGISTRY, NpmDiscovery, NpmDriver, NpmMode,
        NpmProvenance, NpmRegistryProbe, NpmVersionAuthority, OFFICIAL_NPM_REGISTRY, StartupUpdate,
        UpdatePlan,
    };
    use semver::Version;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::time::{Duration, UNIX_EPOCH};

    fn npm_provenance_fixture() -> NpmProvenance {
        NpmProvenance {
            package: "fastctx".to_string(),
            mode: NpmMode::Global,
            node: "node".into(),
            driver: NpmDriver::NodeScript,
            npm_cli: "npm-cli.js".into(),
            launcher: "launcher.js".into(),
            launcher_pid: 42,
            handoff_file: "handoff".into(),
        }
    }

    fn npm_context_fixture(
        source_policy: UpdateSource,
        configured_registry: Option<&str>,
    ) -> NpmCheckContext {
        NpmCheckContext {
            source_policy,
            configured_registry: configured_registry.map(str::to_string),
            configured_registry_failure: configured_registry
                .is_none()
                .then(|| transient("npm config unavailable")),
        }
    }

    fn ready_probe(source_name: &str, registry: &str, version: &str) -> NpmRegistryProbe {
        NpmRegistryProbe {
            source_name: source_name.to_string(),
            registry: registry.to_string(),
            reachable: true,
            latest_version: Some(version.to_string()),
            main_package_ready: true,
            platform_package_ready: true,
            error: None,
            error_kind: None,
        }
    }

    fn npm_discovery_fixture(context: &NpmCheckContext, target_version: &str) -> NpmDiscovery {
        let probe = ready_probe("official npm", OFFICIAL_NPM_REGISTRY, target_version);
        NpmDiscovery {
            source_policy: context.source_policy.as_str().to_string(),
            configured_registry: context.configured_registry.clone(),
            target_version: target_version.to_string(),
            authority: NpmVersionAuthority::Official,
            github_version: Some(target_version.to_string()),
            official_version: Some(target_version.to_string()),
            platform_package: super::platform_npm_package()
                .unwrap_or("@fastctx/unsupported")
                .to_string(),
            probes: vec![probe],
            selected_registry: Some(OFFICIAL_NPM_REGISTRY.to_string()),
            selected_source: Some("official npm".to_string()),
            selection_reason: "fixture selected official npm".to_string(),
        }
    }

    #[derive(Clone, Debug)]
    struct SimulatedRegistry {
        latest: Option<Version>,
        main_package_ready: bool,
        platform_package_ready: bool,
    }

    struct SimulatedRegistryBackend {
        github_latest: Option<Version>,
        platform_package: String,
        registries: BTreeMap<String, SimulatedRegistry>,
    }

    impl NpmProbeBackend for SimulatedRegistryBackend {
        fn latest_github_version(&self) -> Result<Version, CheckFailure> {
            self.github_latest
                .clone()
                .ok_or_else(|| transient("simulated GitHub outage"))
        }

        fn latest_npm_version(
            &self,
            registry: &str,
            _package: &str,
        ) -> Result<Version, CheckFailure> {
            self.registries
                .get(registry)
                .and_then(|state| state.latest.clone())
                .ok_or_else(|| transient(format!("simulated registry outage: {registry}")))
        }

        fn exact_npm_version_exists(
            &self,
            registry: &str,
            package: &str,
            _target_version: &str,
        ) -> Result<bool, CheckFailure> {
            let Some(state) = self.registries.get(registry) else {
                return Err(transient(format!("simulated registry outage: {registry}")));
            };
            if package == "fastctx" {
                Ok(state.main_package_ready)
            } else if package == self.platform_package {
                Ok(state.platform_package_ready)
            } else {
                Ok(false)
            }
        }
    }

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
        assert_eq!(provenance.driver, NpmDriver::NodeScript);
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

        // issue #2: the v0.1.1 launcher shipped marker=1 with an empty FASTCTX_NPM_CLI when it
        // could not find npm beside Node; the empty receipt must be rejected, never trusted.
        let mut environment = valid.clone();
        environment.insert(NPM_CLI_ENV, OsString::new());
        assert!(
            detect(&environment)
                .unwrap_err()
                .contains("did not provide FASTCTX_NPM_CLI"),
            "empty FASTCTX_NPM_CLI must be rejected like the original issue #2 receipt"
        );

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
    fn npm_launcher_v2_accepts_both_drivers_and_makes_unavailable_actionable() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let current = temp.path().join("target/debug/fastctx");
        let node = temp.path().join("node");
        let npm_cli = temp.path().join("npm-cli.js");
        let npm_executable = temp
            .path()
            .join(if cfg!(windows) { "npm.exe" } else { "npm" });
        let launcher = temp.path().join("launcher.js");
        let launcher_pid = 4242;
        let handoff = paths
            .fastctx_dir
            .join("update")
            .join(format!("npm-launcher-{launcher_pid}.handoff"));
        for path in [&node, &npm_cli, &npm_executable, &launcher] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let mut environment = BTreeMap::from([
            (NPM_MARKER_ENV, OsString::from("2")),
            (NPM_PACKAGE_ENV, OsString::from("fastctx")),
            (NPM_MODE_ENV, OsString::from("global")),
            (NODE_ENV, node.into_os_string()),
            (NPM_DRIVER_ENV, OsString::from("node-script")),
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

        let InstallChannel::Npm(provenance) = detect(&environment).unwrap() else {
            panic!("expected npm channel");
        };
        assert_eq!(provenance.driver, NpmDriver::NodeScript);

        environment.insert(NPM_DRIVER_ENV, OsString::from("executable"));
        environment.insert(NPM_CLI_ENV, npm_executable.into_os_string());
        let InstallChannel::Npm(provenance) = detect(&environment).unwrap() else {
            panic!("expected npm channel");
        };
        assert_eq!(provenance.driver, NpmDriver::Executable);

        environment.insert(NPM_DRIVER_ENV, OsString::from("unavailable"));
        environment.remove(NPM_CLI_ENV);
        let error = detect(&environment).unwrap_err();
        assert!(error.contains("no usable npm command"), "{error}");
        assert!(error.contains("npm --version"), "{error}");
        assert!(
            error.contains("npm install --global fastctx --registry=https://registry.npmjs.org/"),
            "{error}"
        );
        assert!(!error.contains(NPM_CLI_ENV), "{error}");
    }

    #[test]
    fn npm_launcher_rejects_unknown_receipt_versions_and_v2_drivers() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let current = temp.path().join("target/debug/fastctx");
        let unknown_version = BTreeMap::from([(NPM_MARKER_ENV, OsString::from("99"))]);
        let detect = |environment: &BTreeMap<&str, OsString>| {
            detect_install_channel(
                &paths,
                &current,
                &|name| environment.get(name).cloned(),
                false,
            )
        };
        let version_error = detect(&unknown_version).unwrap_err();
        assert!(version_error.contains("unsupported receipt version"));
        assert!(version_error.contains("registry.npmjs.org"));

        let node = temp.path().join("node");
        let launcher = temp.path().join("launcher.js");
        let launcher_pid = 4242;
        for path in [&node, &launcher] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let handoff = paths
            .fastctx_dir
            .join("update")
            .join(format!("npm-launcher-{launcher_pid}.handoff"));
        let invalid_driver = BTreeMap::from([
            (NPM_MARKER_ENV, OsString::from("2")),
            (NPM_PACKAGE_ENV, OsString::from("fastctx")),
            (NPM_MODE_ENV, OsString::from("global")),
            (NODE_ENV, node.into_os_string()),
            (NPM_DRIVER_ENV, OsString::from("shell")),
            (NPM_LAUNCHER_ENV, launcher.into_os_string()),
            (
                NPM_LAUNCHER_PID_ENV,
                OsString::from(launcher_pid.to_string()),
            ),
            (NPM_HANDOFF_ENV, handoff.into_os_string()),
        ]);
        assert!(
            detect(&invalid_driver)
                .unwrap_err()
                .contains("unsupported npm driver")
        );
    }

    #[test]
    fn npm_check_bypasses_the_shared_cache_and_pins_the_selected_registry() {
        let arguments = npm_view_arguments(
            "fastctx@0.2.0",
            "https://registry.example.test/custom/",
            std::path::Path::new("/isolated/cache"),
        )
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "view",
                "fastctx@0.2.0",
                "version",
                "--json",
                "--prefer-online",
                "--registry",
                "https://registry.example.test/custom/",
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
    fn registry_urls_are_normalized_without_accepting_credentials_or_query_state() {
        assert_eq!(
            normalize_registry_url("https://registry.example.test/npm".to_string()).unwrap(),
            "https://registry.example.test/npm/"
        );
        assert_eq!(
            normalize_registry_url("http://localhost:4873/".to_string()).unwrap(),
            "http://localhost:4873/"
        );
        for value in [
            "file:///tmp/registry",
            "https://user:secret@registry.example.test/",
            "https://registry.example.test/?token=secret",
            "not a URL",
        ] {
            assert!(
                normalize_registry_url(value.to_string()).is_err(),
                "{value}"
            );
        }
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
    fn official_version_authority_cannot_be_elevated_by_a_newer_mirror() {
        let mirror = RegistryLatest {
            candidate: RegistryCandidate {
                source_name: "npmmirror".to_string(),
                registry: NPMMIRROR_REGISTRY.to_string(),
                selectable: true,
            },
            result: Ok(Version::new(9, 0, 0)),
        };
        let (target, authority) = authoritative_npm_target(
            &Ok(Version::new(0, 2, 0)),
            &Ok(Version::new(0, 1, 9)),
            &[mirror],
        )
        .unwrap();
        assert_eq!(target, Version::new(0, 2, 0));
        assert_eq!(authority, NpmVersionAuthority::Official);
    }

    #[test]
    fn mirror_is_a_version_signal_only_when_both_official_channels_are_unavailable() {
        let mirror = RegistryLatest {
            candidate: RegistryCandidate {
                source_name: "npmmirror".to_string(),
                registry: NPMMIRROR_REGISTRY.to_string(),
                selectable: true,
            },
            result: Ok(Version::new(0, 3, 0)),
        };
        let (target, authority) = authoritative_npm_target(
            &Err(transient("GitHub unavailable")),
            &Err(transient("official npm unavailable")),
            &[mirror],
        )
        .unwrap();
        assert_eq!(target, Version::new(0, 3, 0));
        assert_eq!(authority, NpmVersionAuthority::MirrorFallback);
    }

    #[test]
    fn auto_registry_candidates_follow_the_contract_order_and_deduplicate() {
        let cases = [
            (
                Some(OFFICIAL_NPM_REGISTRY),
                vec![OFFICIAL_NPM_REGISTRY, NPMMIRROR_REGISTRY],
            ),
            (
                Some(NPMMIRROR_REGISTRY),
                vec![NPMMIRROR_REGISTRY, OFFICIAL_NPM_REGISTRY],
            ),
            (
                Some("https://registry.example.test/custom/"),
                vec![
                    "https://registry.example.test/custom/",
                    OFFICIAL_NPM_REGISTRY,
                    NPMMIRROR_REGISTRY,
                ],
            ),
            (None, vec![OFFICIAL_NPM_REGISTRY, NPMMIRROR_REGISTRY]),
        ];
        for (configured, expected) in cases {
            let context = npm_context_fixture(UpdateSource::Auto, configured);
            let actual = registry_candidates(&context)
                .unwrap()
                .into_iter()
                .map(|candidate| candidate.registry)
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "configured={configured:?}");
        }
    }

    #[test]
    fn isolated_registry_matrix_selects_the_first_complete_auto_source() {
        struct Scenario {
            name: &'static str,
            configured_registry: Option<&'static str>,
            mirror_ready: bool,
            official_ready: bool,
            expected_registry: &'static str,
        }

        let scenarios = [
            Scenario {
                name: "npm config is official",
                configured_registry: Some(OFFICIAL_NPM_REGISTRY),
                mirror_ready: true,
                official_ready: true,
                expected_registry: OFFICIAL_NPM_REGISTRY,
            },
            Scenario {
                name: "npm config is a complete mirror",
                configured_registry: Some(NPMMIRROR_REGISTRY),
                mirror_ready: true,
                official_ready: true,
                expected_registry: NPMMIRROR_REGISTRY,
            },
            Scenario {
                name: "configured mirror is still propagating",
                configured_registry: Some(NPMMIRROR_REGISTRY),
                mirror_ready: false,
                official_ready: true,
                expected_registry: OFFICIAL_NPM_REGISTRY,
            },
            Scenario {
                name: "npm config source is unavailable",
                configured_registry: None,
                mirror_ready: true,
                official_ready: true,
                expected_registry: OFFICIAL_NPM_REGISTRY,
            },
        ];

        for scenario in scenarios {
            let context = npm_context_fixture(UpdateSource::Auto, scenario.configured_registry);
            let platform_package = super::platform_npm_package().unwrap().to_string();
            let registry_state = |ready| SimulatedRegistry {
                latest: Some(Version::new(0, 2, 0)),
                main_package_ready: ready,
                platform_package_ready: ready,
            };
            let backend = SimulatedRegistryBackend {
                github_latest: Some(Version::new(0, 2, 0)),
                platform_package,
                registries: BTreeMap::from([
                    (
                        OFFICIAL_NPM_REGISTRY.to_string(),
                        registry_state(scenario.official_ready),
                    ),
                    (
                        NPMMIRROR_REGISTRY.to_string(),
                        registry_state(scenario.mirror_ready),
                    ),
                ]),
            };
            let outcome = probe_npm_channel_with_backend(
                &npm_provenance_fixture(),
                &Version::new(0, 1, 0),
                &context,
                &backend,
            )
            .unwrap();
            let CachedOutcome::NpmAvailable { discovery } = outcome else {
                panic!("{} did not produce an installable update", scenario.name);
            };
            assert_eq!(
                discovery.selected_registry.as_deref(),
                Some(scenario.expected_registry),
                "{}",
                scenario.name
            );
        }
    }

    #[test]
    fn every_source_policy_runs_the_shared_probe_cache_and_four_state_pipeline() {
        #[derive(Clone, Copy, Debug)]
        enum ExpectedState {
            Current,
            Available,
            Pending,
            Failed,
        }

        let configured = "https://registry.example.test/custom/";
        let policies = [
            (UpdateSource::Auto, Some(configured), configured),
            (UpdateSource::NpmConfig, Some(configured), configured),
            (UpdateSource::Official, None, OFFICIAL_NPM_REGISTRY),
            (UpdateSource::Npmmirror, None, NPMMIRROR_REGISTRY),
        ];
        let states = [
            ExpectedState::Current,
            ExpectedState::Available,
            ExpectedState::Pending,
            ExpectedState::Failed,
        ];
        let temp = tempfile::tempdir().unwrap();
        let current_version = Version::new(0, 1, 1);
        let platform_package = super::platform_npm_package().unwrap().to_string();

        for (policy, configured_registry, expected_registry) in policies {
            for state in states {
                let target_version = match state {
                    ExpectedState::Current => Version::new(0, 1, 1),
                    _ => Version::new(0, 2, 0),
                };
                let ready = matches!(state, ExpectedState::Current | ExpectedState::Available);
                let reachable = !matches!(state, ExpectedState::Failed);
                let registry_state = |latest: Version| SimulatedRegistry {
                    latest: reachable.then_some(latest),
                    main_package_ready: ready,
                    platform_package_ready: ready,
                };
                let backend = SimulatedRegistryBackend {
                    github_latest: reachable.then_some(target_version.clone()),
                    platform_package: platform_package.clone(),
                    registries: BTreeMap::from([
                        (
                            configured.to_string(),
                            registry_state(target_version.clone()),
                        ),
                        (
                            OFFICIAL_NPM_REGISTRY.to_string(),
                            registry_state(target_version.clone()),
                        ),
                        (
                            NPMMIRROR_REGISTRY.to_string(),
                            registry_state(Version::new(9, 0, 0)),
                        ),
                    ]),
                };
                let context = npm_context_fixture(policy, configured_registry);
                let mut provenance = npm_provenance_fixture();
                provenance.driver = NpmDriver::Executable;
                provenance.npm_cli = "npm-driver".into();
                let channel = InstallChannel::Npm(provenance.clone());
                let state_name = format!("{state:?}").to_ascii_lowercase();
                let channel_key = format!("matrix-{}-{state_name}", policy.as_str());
                let result = resolve_with_cache(
                    CacheResolution {
                        channel: &channel,
                        channel_key: &channel_key,
                        current_version: "0.1.1",
                        directory: temp.path(),
                        force: true,
                        checked_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                        npm_context: Some(&context),
                    },
                    || {
                        probe_npm_channel_with_backend(
                            &provenance,
                            &current_version,
                            &context,
                            &backend,
                        )
                    },
                );

                let discovery = match (state, result) {
                    (ExpectedState::Current, StartupUpdate::NpmCurrent { discovery }) => discovery,
                    (ExpectedState::Available, StartupUpdate::Available(plan)) => {
                        let UpdatePlan::Npm {
                            provenance,
                            discovery,
                            ..
                        } = *plan
                        else {
                            panic!("{} {state_name} returned the wrong plan", policy.as_str());
                        };
                        assert_eq!(provenance.driver, NpmDriver::Executable);
                        discovery
                    }
                    (ExpectedState::Pending, StartupUpdate::NpmPending { discovery, .. }) => {
                        discovery
                    }
                    (ExpectedState::Failed, StartupUpdate::Failed(failure)) => {
                        assert_eq!(failure.kind, CheckFailureKind::Transient);
                        assert!(
                            failure
                                .message
                                .contains("no npm version authority was reachable")
                        );
                        continue;
                    }
                    (_, actual) => panic!(
                        "{} {state_name} produced the wrong startup state: {actual:?}",
                        policy.as_str()
                    ),
                };

                assert_eq!(discovery.source_policy, policy.as_str());
                assert_eq!(discovery.target_version, target_version.to_string());
                assert_eq!(discovery.authority, NpmVersionAuthority::Official);
                assert_eq!(
                    discovery.selected_registry.as_deref(),
                    ready.then_some(expected_registry)
                );
                let actual_registries = discovery
                    .probes
                    .iter()
                    .map(|probe| probe.registry.as_str())
                    .collect::<Vec<_>>();
                let expected_registries = match policy {
                    UpdateSource::Auto => {
                        vec![configured, OFFICIAL_NPM_REGISTRY, NPMMIRROR_REGISTRY]
                    }
                    UpdateSource::NpmConfig => vec![configured, OFFICIAL_NPM_REGISTRY],
                    UpdateSource::Official => vec![OFFICIAL_NPM_REGISTRY],
                    UpdateSource::Npmmirror => {
                        vec![NPMMIRROR_REGISTRY, OFFICIAL_NPM_REGISTRY]
                    }
                };
                assert_eq!(actual_registries, expected_registries);
            }
        }

        let fallback_context = npm_context_fixture(UpdateSource::Npmmirror, None);
        let fallback_backend = SimulatedRegistryBackend {
            github_latest: None,
            platform_package,
            registries: BTreeMap::from([
                (
                    OFFICIAL_NPM_REGISTRY.to_string(),
                    SimulatedRegistry {
                        latest: None,
                        main_package_ready: false,
                        platform_package_ready: false,
                    },
                ),
                (
                    NPMMIRROR_REGISTRY.to_string(),
                    SimulatedRegistry {
                        latest: Some(Version::new(0, 3, 0)),
                        main_package_ready: true,
                        platform_package_ready: true,
                    },
                ),
            ]),
        };
        let fallback = probe_npm_channel_with_backend(
            &npm_provenance_fixture(),
            &current_version,
            &fallback_context,
            &fallback_backend,
        )
        .unwrap();
        let CachedOutcome::NpmAvailable { discovery } = fallback else {
            panic!("a ready mirror must be usable when both official authorities are unavailable");
        };
        assert_eq!(discovery.target_version, "0.3.0");
        assert_eq!(discovery.authority, NpmVersionAuthority::MirrorFallback);
        assert_eq!(
            discovery.selected_registry.as_deref(),
            Some(NPMMIRROR_REGISTRY)
        );
        assert_eq!(
            super::GITHUB_LATEST_URL,
            "https://github.com/yc-duan/fastctx/releases/latest"
        );
        let forbidden_api_host = ["api", ".github.com"].concat();
        assert!(!super::GITHUB_LATEST_URL.contains(&forbidden_api_host));
        assert!(!super::GITHUB_RELEASE_BASE.contains(&forbidden_api_host));
    }

    #[test]
    fn strict_source_policies_never_fall_through_to_another_registry() {
        let configured = "https://registry.example.test/custom/";
        for (policy, expected) in [
            (UpdateSource::NpmConfig, configured),
            (UpdateSource::Official, OFFICIAL_NPM_REGISTRY),
            (UpdateSource::Npmmirror, NPMMIRROR_REGISTRY),
        ] {
            let context = npm_context_fixture(policy, Some(configured));
            let candidates = registry_candidates(&context).unwrap();
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].registry, expected);
        }
        let unavailable = npm_context_fixture(UpdateSource::NpmConfig, None);
        assert!(
            registry_candidates(&unavailable)
                .unwrap_err()
                .message
                .contains("npm config unavailable")
        );
    }

    #[test]
    fn strict_incomplete_source_stays_pending_even_when_another_registry_is_ready() {
        let context = npm_context_fixture(UpdateSource::Npmmirror, None);
        let candidates = registry_candidates(&context).unwrap();
        let mut mirror = ready_probe("npmmirror", NPMMIRROR_REGISTRY, "0.2.0");
        mirror.platform_package_ready = false;
        let official = ready_probe("official npm", OFFICIAL_NPM_REGISTRY, "0.2.0");

        let outcome = build_npm_outcome(
            &context,
            &Version::new(0, 1, 0),
            &candidates,
            Version::new(0, 2, 0),
            NpmVersionAuthority::Official,
            Some("0.2.0".to_string()),
            Some("0.2.0".to_string()),
            "@fastctx/test-platform",
            vec![mirror, official],
        );
        let CachedOutcome::NpmPending { discovery } = outcome else {
            panic!("a strict incomplete source must not fall through");
        };
        assert!(discovery.selected_registry.is_none());
        assert_eq!(
            discovery.selection_reason,
            "the configured source is reachable but not complete yet"
        );
    }

    #[test]
    fn selection_skips_a_half_installed_source_and_uses_the_next_complete_candidate() {
        let candidates = vec![
            RegistryCandidate {
                source_name: "npm config".to_string(),
                registry: NPMMIRROR_REGISTRY.to_string(),
                selectable: true,
            },
            RegistryCandidate {
                source_name: "official npm".to_string(),
                registry: OFFICIAL_NPM_REGISTRY.to_string(),
                selectable: true,
            },
        ];
        let mut mirror = ready_probe("npm config", NPMMIRROR_REGISTRY, "0.2.0");
        mirror.platform_package_ready = false;
        let official = ready_probe("official npm", OFFICIAL_NPM_REGISTRY, "0.2.0");
        assert_eq!(
            select_ready_candidate(&candidates, &[mirror, official]),
            Some((
                OFFICIAL_NPM_REGISTRY.to_string(),
                "official npm".to_string()
            ))
        );
    }

    #[test]
    fn manual_checks_bypass_both_automatic_check_switches() {
        for (force, environment_disabled, auto_check, expected) in [
            (false, false, true, false),
            (false, true, true, true),
            (false, false, false, true),
            (false, true, false, true),
            (true, true, false, false),
        ] {
            assert_eq!(
                automatic_check_disabled(force, environment_disabled, auto_check),
                expected
            );
        }
    }

    #[test]
    fn auto_check_false_is_a_rejecting_network_oracle_and_manual_check_still_runs() {
        let automatic = run_update_check_if_enabled(false, false, false, || {
            panic!("disabled automatic checks must never reach the injected network probe")
        });
        assert_eq!(automatic, None::<()>);

        let environment_disabled = run_update_check_if_enabled(false, true, true, || {
            panic!("the environment kill switch must never reach the injected network probe")
        });
        assert_eq!(environment_disabled, None::<()>);

        let calls = std::cell::Cell::new(0);
        let manual = run_update_check_if_enabled(true, true, false, || {
            calls.set(calls.get() + 1);
            42
        });
        assert_eq!(manual, Some(42));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn ttl_hit_is_a_zero_probe_oracle_and_force_bypasses_it() {
        let temp = tempfile::tempdir().unwrap();
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let channel = InstallChannel::Npm(npm_provenance_fixture());
        let context = npm_context_fixture(UpdateSource::Official, None);
        let discovery = npm_discovery_fixture(&context, "0.2.0");
        cache::record_success(
            temp.path(),
            "npm-fastctx",
            "0.1.0",
            checked_at,
            &CachedOutcome::NpmAvailable { discovery },
        )
        .unwrap();

        let result = resolve_with_cache(
            CacheResolution {
                channel: &channel,
                channel_key: "npm-fastctx",
                current_version: "0.1.0",
                directory: temp.path(),
                force: false,
                checked_at: checked_at + SUCCESS_TTL,
                npm_context: Some(&context),
            },
            || panic!("a fresh cache hit must not call the network probe"),
        );
        assert!(matches!(result, StartupUpdate::Available(_)));

        let calls = std::cell::Cell::new(0);
        let result = resolve_with_cache(
            CacheResolution {
                channel: &channel,
                channel_key: "npm-fastctx",
                current_version: "0.1.0",
                directory: temp.path(),
                force: true,
                checked_at: checked_at + Duration::from_secs(1),
                npm_context: Some(&context),
            },
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
        let channel = InstallChannel::Npm(npm_provenance_fixture());
        let context = npm_context_fixture(UpdateSource::Official, None);
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
            CacheResolution {
                channel: &channel,
                channel_key: "npm-fastctx",
                current_version: "0.1.0",
                directory: temp.path(),
                force: false,
                checked_at: checked_at + SUCCESS_TTL + Duration::from_secs(1),
                npm_context: Some(&context),
            },
            || {
                calls.set(calls.get() + 1);
                Ok(CachedOutcome::NpmAvailable {
                    discovery: npm_discovery_fixture(&context, "0.2.0"),
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
            CacheResolution {
                channel: &InstallChannel::GithubRelease,
                channel_key: "github-release",
                current_version: "0.1.0",
                directory: temp.path(),
                force: false,
                checked_at,
                npm_context: None,
            },
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
