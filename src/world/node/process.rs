//! Start a node for a user whose login service manager is unavailable.

use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;

/// Starts this build independently of the TUI; automatic login startup remains a separate
/// service-manager operation. The node's own log captures startup and link failures.
pub(crate) fn start(paths: &ControlPaths) -> Result<String, String> {
    let world = WorldPaths::from_control(paths);
    world.ensure()?;
    #[cfg(windows)]
    let pid = super::process_windows::start(paths)?;
    #[cfg(unix)]
    let pid = {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let executable =
            std::env::current_exe().map_err(|error| format!("Cannot locate FastCtx: {error}"))?;
        let mut command = crate::process_policy::noninteractive_command(executable);
        command
            .args(["node", "run", "--home"])
            .arg(&paths.home)
            .current_dir(&paths.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe and the child touches no inherited Rust state.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        command
            .spawn()
            .map_err(|error| format!("Cannot start the node: {error}"))?
            .id()
    };
    Ok(format!(
        "Started node pid {pid} without a service registration. Log: {}",
        crate::paths::display_path(&world.dir.join("node.log"))
    ))
}
