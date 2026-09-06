//! User-level node services, scoped by OS identity and explicit enrollment HOME.
//!
//! New registrations carry `node run --home` so an OS login cannot silently switch Worlds.
//! Legacy unscoped registrations are managed only in this OS user's default profile.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_task;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

use crate::control::paths::ControlPaths;
use std::path::PathBuf;
use std::process::Command;

/// Uses the managed binary only if it is the same build; admin endpoints include build id.
pub(crate) fn service_binary(paths: &ControlPaths) -> Result<PathBuf, String> {
    let current =
        std::env::current_exe().map_err(|error| format!("Cannot locate FastCtx: {error}"))?;
    if paths.installed_binary.is_file()
        && crate::world::remote_edit::sha256_of(&paths.installed_binary)?
            == crate::world::remote_edit::sha256_of(&current)?
    {
        Ok(paths.installed_binary.clone())
    } else {
        Ok(current)
    }
}

fn run_checked(mut command: Command, what: &str) -> Result<String, String> {
    crate::process_policy::apply_noninteractive_policy(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("Cannot {what}: {error}"))?;
    let stdout = command_text(&output.stdout)?;
    let stderr = command_text(&output.stderr)?;
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "Cannot {what}: {}",
            if stderr.is_empty() { stdout } else { stderr }
        ))
    }
}

fn command_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe]) || (bytes.len() > 3 && bytes[1] == 0 && bytes[3] == 0) {
        let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
        if !bytes.len().is_multiple_of(2) {
            return Err("The service manager returned incomplete UTF-16 output.".into());
        }
        let units = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units)
            .map(|text| text.trim().to_string())
            .map_err(|error| format!("Cannot decode service-manager output: {error}"))
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        Ok(text.trim_start_matches('\u{feff}').trim().to_string())
    } else {
        #[cfg(windows)]
        {
            windows_task::command_text(bytes)
        }
        #[cfg(not(windows))]
        {
            Err("The service manager returned invalid UTF-8 output.".into())
        }
    }
}

/// Registers this profile for automatic startup and starts this build.
pub(crate) fn install(paths: &ControlPaths, run_as_user: Option<&str>) -> Result<String, String> {
    platform::install(paths, run_as_user)
}
/// Starts an existing service without interrupting it.
pub(crate) fn start(paths: &ControlPaths) -> Result<String, String> {
    platform::start(paths)
}
/// Updates this profile's service to the current build, then restarts it.
pub(crate) fn restart(paths: &ControlPaths) -> Result<String, String> {
    platform::install(paths, None)
}
/// Stops this profile while preserving its automatic startup registration.
pub(crate) fn stop(paths: &ControlPaths) -> Result<String, String> {
    platform::stop(paths)
}
/// Stops this profile and removes its registration, retaining enrollment files.
pub(crate) fn uninstall(paths: &ControlPaths) -> Result<String, String> {
    platform::uninstall(paths)
}
/// Reads registration independently of whether the node currently answers IPC.
pub(crate) fn inspect(paths: &ControlPaths) -> Result<bool, String> {
    platform::inspect(paths)
}
/// Legacy CLI/Doctor convenience; the World TUI uses fallible inspection.
pub(crate) fn is_installed() -> bool {
    ControlPaths::discover()
        .and_then(|paths| inspect(&paths))
        .unwrap_or(false)
}
