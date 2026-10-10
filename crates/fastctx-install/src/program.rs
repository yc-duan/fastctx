//! Executable identity without opening a World or an agent's configuration.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, ensure};
use fastctx_types::renewal::Build;

/// Identifies a modern kernel and verifies its claimed content identity.
/// A released pre-maintenance executable is recognized by its exact version
/// response; protocol zero means that executable did not identify a protocol.
pub fn inspect(path: &Path) -> anyhow::Result<Build> {
    let path = std::fs::canonicalize(path)?;
    let revision = revision(&path)?;
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()));
    if let Some((held, build)) = cache.lock().expect("kernel identity cache").get(&path)
        && *held == revision
    {
        return Ok(build.clone());
    }
    let build = identify(&path)?;
    ensure!(revision == self::revision(&path)?, "kernel changed during identification: {}", path.display());
    cache.lock().expect("kernel identity cache").insert(path, (revision, build.clone()));
    Ok(build)
}

type Revision = fastctx_platform::file_read::Snapshot;
type Cache = std::collections::BTreeMap<PathBuf, (Revision, Build)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn revision(path: &Path) -> std::io::Result<Revision> {
    let file = fastctx_platform::file_read::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "kernel must be a regular file"));
    }
    fastctx_platform::file_read::snapshot(&file, &metadata)
}

fn identify(path: &Path) -> anyhow::Result<Build> {
    let digest = crate::renewal::digest(path)?;
    let identified = command(path).args(["renew", "identify"]).output()?;
    if identified.status.success() {
        let build: Build = serde_json::from_slice(&identified.stdout).context("reading kernel identity")?;
        ensure!(build.digest == digest, "the identified executable changed: {}", path.display());
        semver::Version::parse(&build.version).context("kernel version is not semver")?;
        return Ok(build);
    }
    let version = command(path).arg("--version").output()?;
    ensure!(version.status.success(), "{} cannot report its version", path.display());
    let text = std::str::from_utf8(&version.stdout)?;
    let version = text.trim().strip_prefix("fastctx ").context("executable is not fastctx")?;
    let released = semver::Version::parse(version)?;
    ensure!(
        released <= semver::Version::new(1, 0, 0),
        "{} does not support kernel identification: exit {}",
        path.display(),
        identified.status
    );
    Ok(Build { version: version.into(), protocol: 0, digest, built: None })
}

/// Identical images are reusable. A different image needs a strictly newer
/// package version, or the same version with a later source time; a build
/// with a source time is later than one without. Neither a digest nor a
/// protocol number orders releases.
pub fn newer(candidate: &Build, previous: &Build) -> anyhow::Result<bool> {
    use std::cmp::Ordering;
    if candidate == previous {
        return Ok(false);
    }
    let offered = semver::Version::parse(&candidate.version)?;
    let current = semver::Version::parse(&previous.version)?;
    let by_time = match (candidate.built, previous.built) {
        (Some(offered), Some(current)) => offered.cmp(&current),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    };
    let order = offered.cmp_precedence(&current).then(by_time);
    ensure!(
        order != Ordering::Equal,
        "different kernels both report {} from the same source time; their order is unknown (installed IPC {}, offered IPC {}); use a newer package version",
        candidate.version,
        previous.protocol,
        candidate.protocol
    );
    Ok(order == Ordering::Greater)
}

pub fn notices(path: &Path) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let output = command(path).args(["renew", "notices"]).output()?;
    ensure!(output.status.success(), "the World program cannot provide its embedded notices");
    let notices: std::collections::BTreeMap<String, String> = serde_json::from_slice(&output.stdout)?;
    ensure!(
        notices.len() == 3
            && ["LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md"].iter().all(|name| notices.contains_key(*name)),
        "embedded notices have an unexpected shape"
    );
    Ok(notices)
}

pub(crate) fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command.stdin(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CLAUDE") || name == "FASTCTX_UPDATE_FINALIZE" {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    command
}
