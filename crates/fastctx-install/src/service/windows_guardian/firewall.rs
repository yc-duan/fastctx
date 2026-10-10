//! The inbound rule that lets other computers reach a hub on this one.
//!
//! A hub is the one part of the World that listens; on Windows its service
//! runs unseen, so the firewall never asks the user and drops every
//! connection from another computer. Installing a hub's service therefore
//! allows inbound connections to the hub's own program, whatever port it is
//! told to listen on, on every network profile: hosting a hub is the user's
//! explicit choice, and a laptop's Wi-Fi is often classed as public. The
//! rule is named after the service, and goes with it.

use std::path::Path;
use std::process::Command;

use anyhow::bail;

fn rule(service: &str) -> String {
    format!("name=fastctx-{service}")
}

/// Whether the service's rule is there. `netsh` answers in the system's
/// language, so only its exit status is read.
pub(super) fn present(service: &str) -> anyhow::Result<bool> {
    let status = Command::new("netsh").args(["advfirewall", "firewall", "show", "rule", &rule(service)]).output()?.status;
    Ok(status.success())
}

/// Allows inbound TCP to `program`, replacing any rule the service had.
pub(super) fn allow(service: &str, program: &Path) -> anyhow::Result<()> {
    remove(service)?;
    let output = Command::new("netsh")
        .args(["advfirewall", "firewall", "add", "rule", &rule(service), "dir=in", "action=allow", "protocol=TCP"])
        .arg(format!("program={}", program.display()))
        .args(["enable=yes", "profile=any"])
        .output()?;
    if !output.status.success() {
        bail!(
            "the firewall would not let other computers reach the hub (netsh exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

/// Removes the service's rule, if it has one.
pub(super) fn remove(service: &str) -> anyhow::Result<()> {
    if !present(service)? {
        return Ok(());
    }
    let output = Command::new("netsh").args(["advfirewall", "firewall", "delete", "rule", &rule(service)]).output()?;
    if !output.status.success() {
        bail!(
            "the hub's firewall rule could not be removed (netsh exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}
