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
    if !cfg!(any(windows, target_os = "linux")) {
        return Ok((Kept::Unsupported, Report::default()));
    }
    let (executable, report) = place_executable(places)?;
    if fastctx_platform::elevation::is_elevated()? {
        Service::new(Kind::Node, &executable, &places.root).install()?;
        return Ok((Kept::Installed, report));
    }
    Ok((elevated(&places.root, &executable)?, report))
}

#[cfg(windows)]
fn elevated(root: &Path, executable: &Path) -> anyhow::Result<Kept> {
    // Named explicitly: when another account's administrator approves, the
    // elevated process runs as that account, not as this user.
    let user = fastctx_platform::current_user_sid()?;
    let args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "node".into(),
        "install".into(),
        "--dir".into(),
        root.into(),
        "--executable".into(),
        executable.into(),
        "--user-sid".into(),
        user.into(),
    ];
    match fastctx_platform::elevation::run_elevated(executable, &args) {
        Ok(0) => Ok(Kept::Installed),
        Ok(code) => anyhow::bail!(
            "installing the node's service failed (exit {code}); `fastctx service node install` run as \
             administrator says why"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(Kept::Declined),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn elevated(root: &Path, executable: &Path) -> anyhow::Result<Kept> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(root)?.uid();
    Ok(Kept::Command(format!(
        "sudo {} service node install --dir {} --executable {} --uid {uid}",
        executable.display(),
        root.display(),
        executable.display()
    )))
}

/// Where a World's marker of its service lives.
pub(super) fn marker(directory: &Path) -> PathBuf {
    WorldDir::under(directory).node_service()
}
