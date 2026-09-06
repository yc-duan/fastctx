//! User service-manager registrations live in the OS profile and carry explicit node HOME.

use super::{run_checked, service_binary};
use crate::control::paths::ControlPaths;
use crate::process_policy::noninteractive_command;
use crate::world::node::service_identity;
use std::path::{Path, PathBuf};

struct Registration {
    name: String,
    file: PathBuf,
}
fn registration(paths: &ControlPaths, legacy: bool) -> Result<Registration, String> {
    let scope = service_identity::scope(paths, None)?;
    let home = service_identity::os_home()?;
    if cfg!(target_os = "macos") {
        let name = if legacy {
            "com.fastctx.node".into()
        } else {
            format!("com.fastctx.node.{scope}")
        };
        Ok(Registration {
            file: home
                .join("Library/LaunchAgents")
                .join(format!("{name}.plist")),
            name,
        })
    } else {
        let name = if legacy {
            "fastctx-node".into()
        } else {
            format!("fastctx-node-{scope}")
        };
        Ok(Registration {
            file: home
                .join(".config/systemd/user")
                .join(format!("{name}.service")),
            name,
        })
    }
}
fn exists(path: &Path) -> Result<bool, String> {
    path.try_exists().map_err(|error| {
        format!(
            "Cannot inspect {}: {error}",
            crate::paths::display_path(path)
        )
    })
}
fn registered(paths: &ControlPaths) -> Result<Option<Registration>, String> {
    let scoped = registration(paths, false)?;
    if exists(&scoped.file)? {
        return Ok(Some(scoped));
    }
    if service_identity::is_default_home(paths)? {
        let legacy = registration(paths, true)?;
        if exists(&legacy.file)? {
            return Ok(Some(legacy));
        }
    }
    Ok(None)
}
fn launchd_loaded(name: &str) -> Result<bool, String> {
    let mut command = noninteractive_command("launchctl");
    command.args(["print", &format!("gui/{}", service_identity::unix_uid())]);
    run_checked(command, "inspect the login LaunchAgent domain").map(|output| {
        output.lines().any(|line| {
            line.split_whitespace()
                .any(|part| part.trim_matches('"') == name)
        })
    })
}
fn stop_registration(reg: &Registration) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        if launchd_loaded(&reg.name)? {
            let mut command = noninteractive_command("launchctl");
            command.args([
                "bootout",
                &format!("gui/{}/{}", service_identity::unix_uid(), reg.name),
            ]);
            run_checked(command, "stop the node LaunchAgent")?;
        }
    } else {
        let mut command = noninteractive_command("systemctl");
        command.args(["--user", "stop", &reg.name]);
        run_checked(command, "stop the node user service")?;
    }
    Ok(())
}
fn uninstall_registration(reg: &Registration) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        stop_registration(reg)?;
    } else {
        let mut command = noninteractive_command("systemctl");
        command.args(["--user", "disable", "--now", &reg.name]);
        run_checked(command, "stop and disable the node user service")?;
    }
    std::fs::remove_file(&reg.file).map_err(|error| {
        format!(
            "Cannot delete {}: {error}",
            crate::paths::display_path(&reg.file)
        )
    })?;
    if !cfg!(target_os = "macos") {
        reload()?;
    }
    Ok(())
}
fn reload() -> Result<(), String> {
    let mut command = noninteractive_command("systemctl");
    command.args(["--user", "daemon-reload"]);
    run_checked(command, "reload the user service manager").map(|_| ())
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn unit_word(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}
fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .filter(|text| !text.contains(['\n', '\r', '\0']))
        .ok_or_else(|| {
            format!(
                "The service manager cannot represent the path {}.",
                crate::paths::display_path(path)
            )
        })
}

pub(super) fn inspect(paths: &ControlPaths) -> Result<bool, String> {
    registered(paths).map(|registration| registration.is_some())
}
pub(super) fn install(paths: &ControlPaths, user: Option<&str>) -> Result<String, String> {
    if user.is_some() {
        return Err("Selecting another service user is supported only on Windows; install in the intended user's login session.".into());
    }
    let reg = registration(paths, false)?;
    let binary = service_binary(paths)?;
    let binary = path_text(&binary)?;
    let home = path_text(&paths.home)?;
    let content = if cfg!(target_os = "macos") {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>node</string><string>run</string><string>--home</string><string>{}</string></array>\n<key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ProcessType</key><string>Background</string>\n</dict></plist>\n",
            xml(&reg.name),
            xml(binary),
            xml(home)
        )
    } else {
        format!(
            "[Unit]\nDescription=FastCtx World node\nAfter=network-online.target\n\n[Service]\nExecStart={} node run --home {}\nRestart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
            unit_word(binary),
            unit_word(home)
        )
    };
    if let Some(existing) = registered(paths)? {
        if existing.name != reg.name {
            uninstall_registration(&existing)?;
        } else {
            stop_registration(&existing)?;
        }
    }
    let parent = reg
        .file
        .parent()
        .ok_or_else(|| "The service path has no parent.".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create the user service directory: {error}"))?;
    crate::world::write_atomic(&reg.file, content.as_bytes())?;
    if cfg!(target_os = "macos") {
        let mut command = noninteractive_command("launchctl");
        command
            .args([
                "bootstrap",
                &format!("gui/{}", service_identity::unix_uid()),
            ])
            .arg(&reg.file);
        run_checked(command, "load the node LaunchAgent")?;
    } else {
        reload()?;
        let mut command = noninteractive_command("systemctl");
        command.args(["--user", "enable", "--now", &reg.name]);
        run_checked(command, "enable and start the node user service")?;
        let mut linger = noninteractive_command("loginctl");
        linger.args(["enable-linger", &service_identity::unix_uid().to_string()]);
        run_checked(
            linger,
            "keep the installed node service running after logout (the service is already installed)",
        )?;
    }
    Ok(format!(
        "Installed and started {} for HOME {}.",
        crate::paths::display_path(&reg.file),
        home
    ))
}
pub(super) fn start(paths: &ControlPaths) -> Result<String, String> {
    let reg = registered(paths)?
        .ok_or_else(|| "No node service is registered for this HOME.".to_string())?;
    if cfg!(target_os = "macos") {
        let mut command = noninteractive_command("launchctl");
        if launchd_loaded(&reg.name)? {
            command.args([
                "kickstart",
                &format!("gui/{}/{}", service_identity::unix_uid(), reg.name),
            ]);
        } else {
            command
                .args([
                    "bootstrap",
                    &format!("gui/{}", service_identity::unix_uid()),
                ])
                .arg(&reg.file);
        }
        run_checked(command, "start the node LaunchAgent")?;
    } else {
        let mut command = noninteractive_command("systemctl");
        command.args(["--user", "start", &reg.name]);
        run_checked(command, "start the node user service")?;
    }
    Ok(format!("Started {}.", reg.name))
}
pub(super) fn stop(paths: &ControlPaths) -> Result<String, String> {
    let Some(reg) = registered(paths)? else {
        return Ok("No node service is registered for this HOME.".into());
    };
    stop_registration(&reg)?;
    Ok(format!("Stopped {}.", reg.name))
}
pub(super) fn uninstall(paths: &ControlPaths) -> Result<String, String> {
    let Some(reg) = registered(paths)? else {
        return Ok("No node service is registered for this HOME.".into());
    };
    uninstall_registration(&reg)?;
    Ok(format!("Removed {}.", reg.name))
}
