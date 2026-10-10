//! Host mutations belong to the selected kernel, including from an old TUI.

use serde::{Deserialize, Serialize};

use super::{Error, Hosts, Places, Report};
use crate::codex::Approval;

/// The private CLI transport preserves approval requests as structured facts.
#[derive(Serialize, Deserialize)]
pub enum Failure {
    OutputLimit { location: String, current: i64, requested: u64 },
    Failed(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Edit(crate::Error::TokenLimitApproval { location, current, requested }) => {
                Self::OutputLimit { location, current, requested }
            }
            error => Self::Failed(error.to_string()),
        }
    }
}

impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        match error {
            Failure::OutputLimit { location, current, requested } => {
                Self::Edit(crate::Error::TokenLimitApproval { location, current, requested })
            }
            Failure::Failed(reason) => Self::Io(reason),
        }
    }
}

pub(super) fn delegate(places: &Places, hosts: Hosts, approval: Approval, into: bool) -> Result<Option<Report>, Error> {
    let installed = places.installed();
    if !installed.is_file() {
        return Ok(None);
    }
    let selected = crate::program::inspect(&installed).map_err(|error| Error::Io(format!("{error:#}")))?;
    // Pre-maintenance kernels have destructive withdrawal code; the modern
    // caller handles their receipts itself instead of invoking that kernel.
    if selected.protocol == 0 {
        return Ok(None);
    }
    if crate::renewal::build().map_err(|error| Error::Io(error.to_string()))? == selected {
        return Ok(None);
    }
    let mut command = std::process::Command::new(&installed);
    command.arg("host-change").env("FASTCTX_HOME", &places.root).stdin(std::process::Stdio::null());
    for (chosen, flag, home) in
        [(hosts.codex, "codex", &places.codex_home), (hosts.claude, "claude", &places.claude_home)]
    {
        if chosen {
            command.arg(format!("--{flag}")).arg(format!("--{flag}-home")).arg(home);
        }
    }
    if into {
        command.arg("--into");
    }
    if matches!(approval, Approval::RaiseOutputLimit) {
        command.arg("--raise-output-limit");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let output = command.output().map_err(|error| Error::Io(error.to_string()))?;
    if !output.status.success() {
        return Err(Error::Io(format!(
            "the selected kernel's host change exited {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let result: Result<Report, Failure> =
        serde_json::from_slice(&output.stdout).map_err(|error| Error::Io(error.to_string()))?;
    result.map(Some).map_err(Into::into)
}
