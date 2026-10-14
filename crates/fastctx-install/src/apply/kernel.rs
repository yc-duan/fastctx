//! Host changes use the selected kernel, or the caller's receipt handler
//! when that kernel has no delegation command.

use serde::{Deserialize, Serialize};

use super::{Edits, Error, Hosts, Places, Report};
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

pub(super) fn delegate(
    places: &Places,
    hosts: Hosts,
    approval: Approval,
    edits: Edits,
    into: bool,
) -> Result<Option<Report>, Error> {
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
    if !supports_host_change(places)? {
        return Ok(None);
    }
    let mut command = crate::program::command(&installed);
    command.arg("host-change").env("FASTCTX_HOME", &places.root);
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
    if edits == Edits::TakeBack {
        command.arg("--take-back");
    }
    // A delegated mutation must end with its caller.
    #[cfg(windows)]
    command.env("FASTCTX_FORWARDED_BY", std::process::id().to_string());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt as _;
        let parent = std::process::id();
        // SAFETY: after fork, only kernel calls and allocation-free error
        // constructors run; no shared memory is accessed.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            })
        };
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

fn supports_host_change(places: &Places) -> Result<bool, Error> {
    // The maintenance protocol predates host-change. Probe the parser without
    // attempting a mutation; only its explicit unknown-command rejection
    // proves absence. Startup failures and other errors must remain errors.
    let output = crate::program::command(&places.installed())
        .args(["host-change", "--help"])
        .env("FASTCTX_HOME", &places.root)
        .env("NO_COLOR", "1")
        .output()
        .map_err(|error| Error::Io(format!("probing the selected kernel's host change: {error}")))?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(2)
        && output.stdout.is_empty()
        && String::from_utf8_lossy(&output.stderr).lines().next()
            == Some("error: unrecognized subcommand 'host-change'")
    {
        return Ok(false);
    }
    Err(Error::Io(format!(
        "the selected kernel's host change probe exited {}: {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}
