//! Keeping a machine connected once it is in a World (`world-network.md`,
//! 加入了就一直连着): a system service starts its node at boot, before
//! anyone signs in, and brings it back whenever it stops. Installing one
//! takes administrator rights, asked for once. Every interface that joins a
//! World does this the same way; the node that served until then is the
//! caller's to ask to hand over.

use std::path::{Path, PathBuf};

use fastctx_types::layout::WorldDir;

use super::{Kind, Service};
use crate::apply::{Places, Report, place_executable};

/// What came of keeping this machine connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// A service already keeps this World's node.
    Already,
    /// The service is installed now; the node that served until now should
    /// hand the World over.
    Installed,
    /// The person at the screen declined to grant the rights.
    Declined,
    /// This system has no service for the node: it runs while the user is
    /// signed in.
    Unsupported,
    /// The rights are asked for with this command, run by the user.
    Command(String),
}

/// Installs the node's service for the World under `places.root`, run from
/// the stable copy of fastctx, for the user running this; with what placing
/// that copy left behind.
pub fn keep_connected(places: &Places) -> anyhow::Result<(Kept, Report)> {
    if WorldDir::under(&places.root).node_service().exists() {
        return Ok((Kept::Already, Report::default()));
    }
    if !cfg!(any(windows, target_os = "linux", target_os = "macos")) {
        return Ok((Kept::Unsupported, Report::default()));
    }
    let (executable, report) = place_executable(places)?;
    // A macOS node is a LaunchAgent of the user's own, installed as the user.
    if cfg!(target_os = "macos") || fastctx_platform::elevation::is_elevated()? {
        Service::new(Kind::Node, &executable, &places.root).install()?;
        return Ok((Kept::Installed, report));
    }
    Ok((elevated("install", &places.root, &executable)?, report))
}

/// What came of no longer keeping this machine connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Released {
    /// No service kept this World's node.
    NotKept,
    /// The service is removed, and its node stopped with it; the node runs
    /// again when something on this machine starts it.
    Removed,
    /// The person at the screen declined to grant the rights.
    Declined,
    /// The rights are asked for with this command, run by the user.
    Command(String),
}

/// Removes the node's service for the World under `places.root`, with the
/// same rights installing it took.
pub fn stop_keeping(places: &Places) -> anyhow::Result<Released> {
    if !WorldDir::under(&places.root).node_service().exists() {
        return Ok(Released::NotKept);
    }
    let executable = places.installed();
    if cfg!(target_os = "macos") || fastctx_platform::elevation::is_elevated()? {
        Service::new(Kind::Node, &executable, &places.root).uninstall()?;
        return Ok(Released::Removed);
    }
    Ok(match elevated("uninstall", &places.root, &executable)? {
        Kept::Installed => Released::Removed,
        Kept::Declined => Released::Declined,
        Kept::Command(command) => Released::Command(command),
        other => anyhow::bail!("removing the node's service came to {other:?}"),
    })
}

/// Runs `fastctx service node <action>` for this user with administrator
/// rights; `Installed` means the action was done.
#[cfg(windows)]
fn elevated(action: &str, root: &Path, executable: &Path) -> anyhow::Result<Kept> {
    // Named explicitly: when another account's administrator approves, the
    // elevated process runs as that account, not as this user.
    let user = fastctx_platform::current_user_sid()?;
    let args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "node".into(),
        action.into(),
        "--dir".into(),
        root.into(),
        "--executable".into(),
        executable.into(),
        "--user-sid".into(),
        user.into(),
    ];
    match fastctx_platform::elevation::run_elevated(executable, &args) {
        Ok(0) => Ok(Kept::Installed),
        Ok(code) => {
            anyhow::bail!("`fastctx service node {action}` failed (exit {code}); run as administrator, it says why")
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(Kept::Declined),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn elevated(action: &str, root: &Path, executable: &Path) -> anyhow::Result<Kept> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(root)?.uid();
    Ok(Kept::Command(format!(
        "sudo {} service node {action} --dir {} --executable {} --uid {uid}",
        fastctx_platform::shell_quote::path(executable),
        fastctx_platform::shell_quote::path(root),
        fastctx_platform::shell_quote::path(executable)
    )))
}

/// Where a World's marker of its service lives.
pub(super) fn marker(directory: &Path) -> PathBuf {
    WorldDir::under(directory).node_service()
}
