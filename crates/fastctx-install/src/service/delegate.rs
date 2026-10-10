//! A library caller, including the GUI, does not become the SYSTEM guardian.

use std::process::{Command, Stdio};

use anyhow::{Context as _, ensure};

use super::{Service, Status};

pub(super) fn install(service: &Service) -> anyhow::Result<Option<Status>> {
    if std::fs::canonicalize(std::env::current_exe()?)? == std::fs::canonicalize(&service.executable)?
        && crate::renewal::build()? == crate::program::inspect(&service.executable)?
    {
        return Ok(None);
    }
    let mut command = Command::new(&service.executable);
    command.args(["service", service.role(), "install", "--dir"]).arg(&service.directory);
    command.arg("--executable").arg(&service.executable).stdin(Stdio::null());
    if let Some(sid) = &service.user_sid {
        command.arg("--user-sid").arg(sid);
    }
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x08000000);
    let output = command.output().context("starting the selected World kernel to install its service")?;
    ensure!(
        output.status.success(),
        "service installation from the World kernel exited {}: {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(Some(serde_json::from_slice(&output.stdout).context("reading the kernel's service registration")?))
}
