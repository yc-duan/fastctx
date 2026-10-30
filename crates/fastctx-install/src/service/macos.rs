//! launchd keeps the node and the hub on macOS. A node is a LaunchAgent of
//! the user it serves, in that user's `gui` domain: it starts when the user
//! logs in, and needs no administrator to install. A hub, and a node root
//! installed for itself, as a Mac's line run as root does (`world-network.md`,
//! 登录与入网), is a LaunchDaemon in the `system` domain, run as root from the
//! start of the machine. Either is started again whenever it exits.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};
use fastctx_platform::{Permissions, publish};

use super::{Kind, Service, Status, checked};

/// The first line of a definition this module wrote.
const MARKER: &str = "<!-- fastctx service v1 -->";
/// The application a user's node belongs to: its bundle identifier
/// (`gui/src-tauri/tauri.conf.json`).
const APPLICATION: &str = "dev.fastctx.world";

pub(super) fn status(service: &Service) -> anyhow::Result<Status> {
    let installed = definition_path(service)?.try_exists()?;
    observed(service, installed)
}

pub(super) fn definition_current(service: &Service) -> anyhow::Result<bool> {
    let path = definition_path(service)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(bytes == definition(service)?.as_bytes()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("reading LaunchAgent {}", path.display())),
    }
}

pub(super) fn install(service: &Service) -> anyhow::Result<Status> {
    if !service.executable.is_file() || !service.directory.is_dir() {
        bail!("the executable and service data directory must exist before installation");
    }
    let path = definition_path(service)?;
    let content = definition(service)?;
    let before = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let target = target(service);
    // Enabled first: launchd loads no service it holds disabled, as a held
    // one is (`hold`), and enabling one not loaded only clears that mark.
    launchctl(&["enable", &target])?;
    if before.as_deref() != Some(content.as_bytes()) {
        // A changed definition is loaded again; an unloaded one is not an
        // error here.
        if before.is_some() {
            let _ = launchctl(&["bootout", &target]);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        publish(&path, content.as_bytes(), Permissions::KeepExisting)?;
        bootstrap(service, &path)?;
    } else if !loaded(&target) {
        bootstrap(service, &path)?;
    }
    // Started if it is not running; one that runs is left alone.
    launchctl(&["kickstart", &target])?;
    observed(service, true)
}

pub(super) fn uninstall(service: &Service) -> anyhow::Result<Status> {
    let path = definition_path(service)?;
    let target = target(service);
    if loaded(&target) {
        launchctl(&["bootout", &target])?;
    }
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    observed(service, false)
}

/// Stops the service and keeps it stopped: unloaded, launchd no longer keeps
/// it alive, and disabled, it is not loaded at the next login or boot. Its
/// definition stays.
pub(super) fn hold(service: &Service) -> anyhow::Result<Status> {
    let target = target(service);
    launchctl(&["disable", &target])?;
    if loaded(&target) {
        launchctl(&["bootout", &target])?;
    }
    observed(service, true)
}

/// Loads, enables and starts it again, as installing it does.
pub(super) fn release(service: &Service) -> anyhow::Result<Status> {
    install(service)
}

/// Whether this process may start the service again: a user's own agent
/// by that user, a daemon of the machine by root alone. A daemon a user may
/// not restart starts again by itself on a newer World program
/// (`fastctx_hub::renewal`).
pub(super) fn may_restart(service: &Service) -> anyhow::Result<()> {
    if daemon(service) && !fastctx_platform::elevation::is_elevated()? {
        bail!("{} is a daemon of this Mac, which only root restarts", label(service));
    }
    Ok(())
}

/// Starts the service's program again now, as after a new image was put in
/// place.
pub(super) fn restart(service: &Service) -> anyhow::Result<()> {
    launchctl(&["kickstart", "-k", &target(service)])?;
    Ok(())
}

/// The registrations of fastctx under `installation`, read from the
/// definitions this module wrote.
pub(super) fn registrations(installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<super::Registration>> {
    let mut found = Vec::new();
    let places = [installation.home.join("Library/LaunchAgents"), PathBuf::from("/Library/LaunchDaemons")];
    for place in places {
        for entry in super::inventory::entries(&place)? {
            let name = entry.file_name();
            let Some(name) =
                name.to_str().filter(|name| name.starts_with("fastctx-service-") && name.ends_with(".plist"))
            else {
                continue;
            };
            let text = std::fs::read_to_string(entry.path())?;
            if !text.starts_with(MARKER) {
                bail!("{} is not an owned fastctx definition", entry.path().display());
            }
            let words = arguments(&text).with_context(|| format!("{} names no program", entry.path().display()))?;
            let executable = PathBuf::from(words.first().context("the definition names no program")?);
            if !executable.starts_with(installation.root.join("bin")) {
                continue;
            }
            let kind = match words.get(1).map(String::as_str) {
                Some("node") => Kind::Node,
                Some("hub") => Kind::Hub,
                _ => bail!("{} has an unrecognized fastctx role", entry.path().display()),
            };
            let at = words.iter().position(|word| word == "--dir").context("the definition has no directory")?;
            let directory = PathBuf::from(words.get(at + 1).context("the definition's directory is absent")?);
            let mut service = Service::new(kind, &executable, &directory);
            if kind == Kind::Node {
                service.uid = installation.uid;
            }
            if format!("{}.plist", label(&service)) != name {
                bail!("{} does not match its directory and account", entry.path().display());
            }
            found.push(super::Registration {
                name: label(&service),
                kind,
                executable,
                directory,
                artifacts: Vec::new(),
            });
        }
    }
    Ok(found)
}

fn label(service: &Service) -> String {
    service.id(&service.uid.to_le_bytes())
}

fn target(service: &Service) -> String {
    format!("{}/{}", domain(service), label(service))
}

/// Whether launchd runs it as a daemon of the machine: a hub, or root's node.
fn daemon(service: &Service) -> bool {
    service.kind == Kind::Hub || service.uid == 0
}

fn domain(service: &Service) -> String {
    if daemon(service) { "system".to_string() } else { format!("gui/{}", service.uid) }
}

fn definition_path(service: &Service) -> anyhow::Result<PathBuf> {
    let file = format!("{}.plist", label(service));
    Ok(if daemon(service) {
        PathBuf::from("/Library/LaunchDaemons").join(file)
    } else {
        fastctx_platform::user_home()?.join("Library/LaunchAgents").join(file)
    })
}

fn definition(service: &Service) -> anyhow::Result<String> {
    let mut words = vec![service.executable.to_str().context("the executable's path is not UTF-8")?.to_string()];
    words.push(service.role().to_string());
    if service.kind == Kind::Hub {
        words.push("serve".to_string());
    }
    words.push("--dir".to_string());
    words.push(service.directory.to_str().context("the directory's path is not UTF-8")?.to_string());
    let log = service.directory.join(format!("service-{}.log", service.role()));
    let log = log.to_str().context("the log's path is not UTF-8")?;
    let arguments: String = words.iter().map(|word| format!("\t\t<string>{}</string>\n", escape(word))).collect();
    // A user's node belongs to the application: System Settings names it
    // Fastctx among what runs in the background.
    let belongs = if daemon(service) {
        String::new()
    } else {
        format!("\t<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>{APPLICATION}</string>\n\t</array>\n")
    };
    Ok(format!(
        "{MARKER}\n<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key>\n\t<string>{}</string>\n\t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>KeepAlive</key>\n\t<true/>\n\t<key>ThrottleInterval</key>\n\t<integer>1</integer>\n\t<key>ProcessType</key>\n\t<string>Standard</string>\n\t<key>StandardInPath</key>\n\t<string>/dev/null</string>\n\t<key>StandardOutPath</key>\n\t<string>{}</string>\n\t<key>StandardErrorPath</key>\n\t<string>{}</string>\n{belongs}</dict>\n</plist>\n",
        escape(&label(service)),
        escape(log),
        escape(log),
    ))
}

/// The program and its arguments, as a definition this module wrote says.
fn arguments(text: &str) -> Option<Vec<String>> {
    let start = text.find("<key>ProgramArguments</key>")?;
    let rest = &text[start..];
    let array = &rest[rest.find("<array>")? + "<array>".len()..rest.find("</array>")?];
    let mut words = Vec::new();
    let mut rest = array;
    while let Some(open) = rest.find("<string>") {
        let after = &rest[open + "<string>".len()..];
        let close = after.find("</string>")?;
        words.push(unescape(&after[..close]));
        rest = &after[close + "</string>".len()..];
    }
    Some(words)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn bootstrap(service: &Service, path: &Path) -> anyhow::Result<()> {
    let path = path.to_str().context("the definition's path is not UTF-8")?;
    launchctl(&["bootstrap", &domain(service), path])?;
    Ok(())
}

fn loaded(target: &str) -> bool {
    Command::new("launchctl").args(["print", target]).output().is_ok_and(|output| output.status.success())
}

fn launchctl(args: &[&str]) -> anyhow::Result<String> {
    checked(Command::new("launchctl").args(args))
}

/// What launchd says of the service now.
fn observed(service: &Service, installed: bool) -> anyhow::Result<Status> {
    let target = target(service);
    let printed = Command::new("launchctl").args(["print", &target]).output()?;
    let text = String::from_utf8_lossy(&printed.stdout);
    let field =
        |name: &str| text.lines().find_map(|line| line.trim().strip_prefix(name).map(|value| value.trim().to_string()));
    let state = if printed.status.success() {
        field("state = ").unwrap_or_else(|| "loaded".into())
    } else {
        "not loaded".into()
    };
    let last_exit = field("last exit code = ").and_then(|code| code.parse().ok());
    let logs = service.directory.join(format!("service-{}.log", service.role())).display().to_string();
    let disabled = launchctl(&["print-disabled", &domain(service)])?;
    let entry = format!("\"{}\" => true", label(service));
    let enabled = installed && !disabled.lines().any(|line| line.trim().trim_end_matches(',') == entry);
    Ok(Status { name: label(service), installed, enabled, state, last_exit, logs, recovery: None })
}
