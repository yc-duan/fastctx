//! Keeping a hub running on this machine (`world-network.md`, 一个中枢，几个
//! 账号): the computer a World is created on runs its hub as a system
//! service, started at boot and brought back whenever it stops, from the
//! same placed fastctx the node runs from. Installing it takes administrator
//! rights, asked for once; on Windows it also lets other computers reach
//! the hub through the firewall.

use std::path::Path;

use super::{Kept, Kind, Service};
use crate::apply::{Places, place_executable};

/// Has a system service run the hub kept in `dir`.
pub fn keep_hub(places: &Places, dir: &Path) -> anyhow::Result<Kept> {
    if !cfg!(any(windows, target_os = "linux", target_os = "macos")) {
        return Ok(Kept::Unsupported);
    }
    let (executable, _) = place_executable(places)?;
    keep_hub_at(places, dir, &executable)
}

/// Has a system service run the hub kept in `dir` from `executable`, a World
/// program already in place, as on a machine whose node runs from it: placing
/// another would wait for that node to hand over.
pub fn keep_hub_at(places: &Places, dir: &Path, executable: &Path) -> anyhow::Result<Kept> {
    if !cfg!(any(windows, target_os = "linux", target_os = "macos")) {
        return Ok(Kept::Unsupported);
    }
    super::hub_registry::remember(&places.root, dir)?;
    if fastctx_platform::elevation::is_elevated()? {
        Service::new(Kind::Hub, executable, dir).install()?;
        return Ok(Kept::Installed);
    }
    elevated(dir, executable)
}

#[cfg(windows)]
fn elevated(dir: &Path, executable: &Path) -> anyhow::Result<Kept> {
    let args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "hub".into(),
        "install".into(),
        "--dir".into(),
        dir.into(),
        "--executable".into(),
        executable.into(),
    ];
    match fastctx_platform::elevation::run_elevated(executable, &args) {
        Ok(0) => Ok(Kept::Installed),
        Ok(code) => {
            anyhow::bail!("`fastctx service hub install` failed (exit {code}); run as administrator, it says why")
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(Kept::Declined),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn elevated(dir: &Path, executable: &Path) -> anyhow::Result<Kept> {
    Ok(Kept::Command(format!(
        "sudo {} service hub install --dir {} --executable {}",
        fastctx_platform::shell_quote::path(executable),
        fastctx_platform::shell_quote::path(dir),
        fastctx_platform::shell_quote::path(executable)
    )))
}
