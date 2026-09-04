//! Installing `fastctx node run` as a user-level service: a logon scheduled task on Windows,
//! a `systemd --user` unit on Linux, a LaunchAgent on macOS.

use crate::control::paths::ControlPaths;
use std::path::PathBuf;
use std::process::Command;

pub(crate) const WINDOWS_TASK_NAME: &str = "FastCtx Node";
pub(crate) const SYSTEMD_UNIT: &str = "fastctx-node";
pub(crate) const LAUNCHD_LABEL: &str = "com.fastctx.node";

/// The binary the service runs: Apply's stable copy when present, else the current one.
pub(crate) fn service_binary(paths: &ControlPaths) -> Result<PathBuf, String> {
    if paths.installed_binary.is_file() {
        return Ok(paths.installed_binary.clone());
    }
    std::env::current_exe()
        .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))
}

fn run_checked(mut command: Command, what: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("Cannot {what}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "Cannot {what}: {}",
            if stderr.is_empty() { stdout } else { stderr }
        ))
    }
}

/// Registers the service for the current user and starts it.
pub(crate) fn install(paths: &ControlPaths, run_as_user: Option<&str>) -> Result<String, String> {
    let binary = service_binary(paths)?;
    if cfg!(windows) {
        let action = format!("\"{}\" node run", binary.display());
        let mut command = Command::new("schtasks");
        command.args([
            "/Create",
            "/TN",
            WINDOWS_TASK_NAME,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/F",
            "/TR",
            &action,
        ]);
        if let Some(user) = run_as_user {
            command.args(["/RU", user, "/IT"]);
        }
        run_checked(command, "create the logon task")?;
        let mut start = Command::new("schtasks");
        start.args(["/Run", "/TN", WINDOWS_TASK_NAME]);
        run_checked(start, "start the logon task")?;
        return Ok(format!(
            "Registered the logon task \"{WINDOWS_TASK_NAME}\" running {} and started it.",
            crate::paths::display_path(&binary)
        ));
    }
    if cfg!(target_os = "macos") {
        let directory = paths.home.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "Cannot create {}: {error}",
                crate::paths::display_path(&directory)
            )
        })?;
        let plist = directory.join(format!("{LAUNCHD_LABEL}.plist"));
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LAUNCHD_LABEL}</string>\n  <key>ProgramArguments</key><array><string>{}</string><string>node</string><string>run</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Background</string>\n</dict>\n</plist>\n",
            binary.display()
        );
        crate::world::write_atomic(&plist, content.as_bytes())?;
        let uid = unix_uid();
        let mut bootstrap = Command::new("launchctl");
        bootstrap.args(["bootstrap", &format!("gui/{uid}"), &plist.to_string_lossy()]);
        if run_checked(bootstrap, "load the LaunchAgent").is_err() {
            let mut load = Command::new("launchctl");
            load.args(["load", "-w", &plist.to_string_lossy()]);
            run_checked(load, "load the LaunchAgent")?;
        }
        return Ok(format!(
            "Installed the LaunchAgent {} and started it.",
            crate::paths::display_path(&plist)
        ));
    }
    let directory = paths.home.join(".config").join("systemd").join("user");
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "Cannot create {}: {error}",
            crate::paths::display_path(&directory)
        )
    })?;
    let unit = directory.join(format!("{SYSTEMD_UNIT}.service"));
    let content = format!(
        "[Unit]\nDescription=FastCtx World node\nAfter=network-online.target\n\n[Service]\nExecStart={} node run\nRestart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
        binary.display()
    );
    crate::world::write_atomic(&unit, content.as_bytes())?;
    let mut linger = Command::new("loginctl");
    linger.args(["enable-linger"]);
    let _ = linger.output();
    let mut reload = Command::new("systemctl");
    reload.args(["--user", "daemon-reload"]);
    run_checked(
        reload,
        "reload the user systemd manager (is a user session running? try 'loginctl enable-linger' and log in again)",
    )?;
    let mut enable = Command::new("systemctl");
    enable.args(["--user", "enable", "--now", SYSTEMD_UNIT]);
    run_checked(enable, "enable the fastctx-node user unit")?;
    Ok(format!(
        "Installed the user unit {} and started it.",
        crate::paths::display_path(&unit)
    ))
}

/// Stops and removes the service registration.
pub(crate) fn uninstall(paths: &ControlPaths) -> Result<String, String> {
    if cfg!(windows) {
        let mut end = Command::new("schtasks");
        end.args(["/End", "/TN", WINDOWS_TASK_NAME]);
        let _ = end.output();
        let mut delete = Command::new("schtasks");
        delete.args(["/Delete", "/TN", WINDOWS_TASK_NAME, "/F"]);
        run_checked(delete, "delete the logon task")?;
        return Ok(format!("Removed the logon task \"{WINDOWS_TASK_NAME}\"."));
    }
    if cfg!(target_os = "macos") {
        let plist = paths
            .home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist"));
        let uid = unix_uid();
        let mut bootout = Command::new("launchctl");
        bootout.args(["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")]);
        let _ = bootout.output();
        let _ = std::fs::remove_file(&plist);
        return Ok(format!(
            "Removed the LaunchAgent {}.",
            crate::paths::display_path(&plist)
        ));
    }
    let mut disable = Command::new("systemctl");
    disable.args(["--user", "disable", "--now", SYSTEMD_UNIT]);
    let _ = disable.output();
    let unit = paths
        .home
        .join(".config")
        .join("systemd")
        .join("user")
        .join(format!("{SYSTEMD_UNIT}.service"));
    let _ = std::fs::remove_file(&unit);
    let mut reload = Command::new("systemctl");
    reload.args(["--user", "daemon-reload"]);
    let _ = reload.output();
    Ok(format!(
        "Removed the user unit {}.",
        crate::paths::display_path(&unit)
    ))
}

/// Restarts the service so a new binary or configuration takes effect.
pub(crate) fn restart(paths: &ControlPaths) -> Result<String, String> {
    if cfg!(windows) {
        let mut end = Command::new("schtasks");
        end.args(["/End", "/TN", WINDOWS_TASK_NAME]);
        let _ = end.output();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut run = Command::new("schtasks");
        run.args(["/Run", "/TN", WINDOWS_TASK_NAME]);
        run_checked(
            run,
            "start the logon task (is it installed? run 'fastctx node install-service')",
        )?;
        return Ok(format!("Restarted the logon task \"{WINDOWS_TASK_NAME}\"."));
    }
    if cfg!(target_os = "macos") {
        let uid = unix_uid();
        let mut kick = Command::new("launchctl");
        kick.args(["kickstart", "-k", &format!("gui/{uid}/{LAUNCHD_LABEL}")]);
        run_checked(
            kick,
            "restart the LaunchAgent (is it installed? run 'fastctx node install-service')",
        )?;
        return Ok("Restarted the LaunchAgent.".to_string());
    }
    let _ = paths;
    let mut restart = Command::new("systemctl");
    restart.args(["--user", "restart", SYSTEMD_UNIT]);
    run_checked(
        restart,
        "restart the fastctx-node user unit (is it installed? run 'fastctx node install-service')",
    )?;
    Ok("Restarted the fastctx-node user unit.".to_string())
}

/// Stops the service without removing it.
pub(crate) fn stop(paths: &ControlPaths) -> Result<String, String> {
    let _ = paths;
    if cfg!(windows) {
        let mut end = Command::new("schtasks");
        end.args(["/End", "/TN", WINDOWS_TASK_NAME]);
        run_checked(end, "stop the logon task")?;
        return Ok(format!("Stopped the logon task \"{WINDOWS_TASK_NAME}\"."));
    }
    if cfg!(target_os = "macos") {
        let uid = unix_uid();
        let mut kill = Command::new("launchctl");
        kill.args(["kill", "TERM", &format!("gui/{uid}/{LAUNCHD_LABEL}")]);
        run_checked(kill, "stop the LaunchAgent")?;
        return Ok("Stopped the LaunchAgent (it restarts on next login).".to_string());
    }
    let mut stop = Command::new("systemctl");
    stop.args(["--user", "stop", SYSTEMD_UNIT]);
    run_checked(stop, "stop the fastctx-node user unit")?;
    Ok("Stopped the fastctx-node user unit.".to_string())
}

/// Whether the service registration exists, as the platform reports it.
pub(crate) fn is_installed() -> bool {
    if cfg!(windows) {
        return Command::new("schtasks")
            .args(["/Query", "/TN", WINDOWS_TASK_NAME])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
    }
    if cfg!(target_os = "macos") {
        return Command::new("launchctl")
            .args(["print", &format!("gui/{}/{LAUNCHD_LABEL}", unix_uid())])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
    }
    Command::new("systemctl")
        .args(["--user", "is-enabled", SYSTEMD_UNIT])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn unix_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn unix_uid() -> u32 {
    0
}
