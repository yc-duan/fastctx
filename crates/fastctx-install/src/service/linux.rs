//! System units run at boot under an explicit account, without a login or
//! a lingering user manager. systemd retains exit diagnostics in its journal.

use std::fs;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, bail};
use fastctx_platform::FileLock;
use fastctx_platform::atomic::{Permissions, publish};

use super::linux_watchdog::{self, Watchdog};
use super::{Service, Status, checked, transaction};
use serde::{Deserialize, Serialize};

pub(super) fn registrations(installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<super::Registration>> {
    let mut found = Vec::new();
    for entry in super::inventory::entries(std::path::Path::new("/etc/systemd/system"))? {
        let name = entry.file_name();
        let Some(name) =
            name.to_str().filter(|name| name.starts_with("fastctx-service-") && name.ends_with(".executable"))
        else {
            continue;
        };
        let executable = fs::read_link(entry.path())?;
        if !executable.starts_with(installation.root.join("bin")) {
            continue;
        }
        let stem = name.trim_end_matches(".executable");
        let file = entry.path().with_extension("service");
        let text = fs::read_to_string(&file)?;
        let marker = format!("# fastctx service v1: {stem}\n");
        if !text.starts_with(&marker) {
            bail!("{} is not an owned fastctx unit", file.display());
        }
        let uid: u32 =
            text.lines().find_map(|line| line.strip_prefix("User=")).context("unit has no account")?.parse()?;
        let start = text
            .lines()
            .find_map(|line| line.strip_prefix("ExecStart=:").or_else(|| line.strip_prefix("ExecStart=")))
            .context("unit has no command")?;
        let words = unquote(start)?;
        let kind = if words.get(1).is_some_and(|word| word == "node") {
            super::Kind::Node
        } else if words.get(1).is_some_and(|word| word == "hub") {
            super::Kind::Hub
        } else {
            bail!("unit has an unrecognized fastctx role")
        };
        let index = words.iter().position(|word| word == "--dir").context("unit has no directory")?;
        let directory = PathBuf::from(words.get(index + 1).context("unit directory is absent")?);
        if kind == super::Kind::Node && uid != installation.uid {
            continue;
        }
        let mut service = Service::new(kind, &executable, &directory);
        service.uid = uid;
        if service.id(&uid.to_le_bytes()) != stem {
            bail!("unit directory and account do not match its name");
        }
        found.push(super::Registration {
            name: format!("{stem}.service"),
            kind,
            executable,
            directory,
            artifacts: vec![PathBuf::from("/run/lock").join(format!("{stem}.service.lock"))],
        });
    }
    Ok(found)
}

fn unquote(text: &str) -> anyhow::Result<Vec<std::ffi::OsString>> {
    let mut result = Vec::new();
    for word in text.split(' ') {
        let word =
            word.strip_prefix('"').and_then(|word| word.strip_suffix('"')).context("unit argument is not quoted")?;
        let bytes = word.as_bytes();
        let mut decoded = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at..].starts_with(b"\\x") {
                let hex = std::str::from_utf8(bytes.get(at + 2..at + 4).context("unit escape is incomplete")?)?;
                decoded.push(u8::from_str_radix(hex, 16)?);
                at += 4;
            } else if bytes[at..].starts_with(b"%%") {
                decoded.push(b'%');
                at += 2;
            } else if bytes[at..].starts_with(b"$$") {
                decoded.push(b'$');
                at += 2;
            } else {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
        result.push(std::ffi::OsString::from_vec(decoded));
    }
    Ok(result)
}

#[derive(Deserialize, Serialize)]
struct Before {
    unit: Option<Vec<u8>>,
    executable: Option<Vec<u8>>,
    status: Status,
    watchdog: linux_watchdog::Before,
}

struct Registration {
    /// The unit's name without its suffix.
    id: String,
    name: String,
    file: PathBuf,
    marker: String,
    executable: PathBuf,
    watchdog: Watchdog,
}

impl Registration {
    fn new(service: &Service) -> Self {
        let id = service.id(&service.uid.to_le_bytes());
        Self {
            watchdog: Watchdog::new(&format!("{id}.service"), &format!("# fastctx service v1: {id}\n")),
            name: format!("{id}.service"),
            id: id.clone(),
            file: PathBuf::from("/etc/systemd/system").join(format!("{id}.service")),
            marker: format!("# fastctx service v1: {id}\n"),
            executable: PathBuf::from("/etc/systemd/system").join(format!("{id}.executable")),
        }
    }

    fn read(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let metadata = match fs::symlink_metadata(&self.file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() {
            bail!("{} is not a regular fastctx unit file", self.file.display());
        }
        let bytes = fs::read(&self.file)?;
        if !bytes.starts_with(self.marker.as_bytes()) {
            bail!("{} is not owned by this fastctx service", self.file.display());
        }
        Ok(Some(bytes))
    }

    fn status(&self, installed: bool) -> anyhow::Result<Status> {
        if !installed {
            let load = systemctl(&["show", &self.name, "--property=LoadState"])?;
            if load.trim().strip_prefix("LoadState=").unwrap_or(load.trim()) != "not-found" {
                bail!("{} is loaded from another location; refusing to manage it", self.name);
            }
            return Ok(Status {
                name: self.name.clone(),
                installed: false,
                enabled: false,
                state: "absent".into(),
                last_exit: None,
                logs: format!("journalctl --unit {}", self.name),
                recovery: Some(self.watchdog.status()?),
            });
        }
        checked(Command::new("busctl").args([
            "--system",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "LoadUnit",
            "s",
            &self.name,
        ]))?;
        let active = self.property("Unit", "ActiveState")?;
        let sub = self.property("Unit", "SubState")?;
        let enabled = self.property("Unit", "UnitFileState")?;
        let exit = self.property("Service", "ExecMainStatus")?;
        Ok(Status {
            name: self.name.clone(),
            installed,
            enabled: enabled.as_str().context("systemd omitted UnitFileState")? == "enabled",
            state: format!(
                "{}/{}",
                active.as_str().context("systemd omitted ActiveState")?,
                sub.as_str().context("systemd omitted SubState")?
            ),
            last_exit: Some(exit.as_i64().context("systemd omitted ExecMainStatus")?),
            logs: format!("journalctl --unit {}", self.name),
            recovery: Some(self.watchdog.status()?),
        })
    }

    /// systemctl show requests GetAll even with --property. An ExecStart
    /// containing non-UTF-8 argv cannot be serialized as a D-Bus string, so
    /// GetAll fails. Get reads only the requested, representable property.
    fn property(&self, interface: &str, property: &str) -> anyhow::Result<serde_json::Value> {
        let path = format!(
            "/org/freedesktop/systemd1/unit/{}",
            self.name
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() { (byte as char).to_string() } else { format!("_{byte:02x}") }
                })
                .collect::<String>()
        );
        let output = checked(Command::new("busctl").args([
            "--system",
            "get-property",
            "org.freedesktop.systemd1",
            &path,
            &format!("org.freedesktop.systemd1.{interface}"),
            property,
        ]))?;
        busctl_value(&output)
    }
}

pub(super) fn status(service: &Service) -> anyhow::Result<Status> {
    let reg = Registration::new(service);
    transaction::require_complete(&reg.file.with_extension("transaction"))?;
    reg.status(reg.read()?.is_some())
}

pub(super) fn definition_current(service: &Service) -> anyhow::Result<bool> {
    let reg = Registration::new(service);
    let content = definition(service, &reg, manager_version()?)?;
    Ok(reg.read()?.as_deref() == Some(content.as_bytes())
        && read_link(&reg.executable)?.as_deref() == Some(service.executable.as_path())
        && reg.watchdog.current(&reg.name, manager_version()?)?)
}

pub(super) fn main_pid(service: &Service) -> anyhow::Result<u64> {
    let pid = Registration::new(service).property("Service", "MainPID")?;
    pid.as_u64().context("systemd omitted the service's MainPID")
}

pub(super) fn install(service: &Service) -> anyhow::Result<Status> {
    let version = manager_version()?;
    let reg = Registration::new(service);
    let _lock = FileLock::acquire(&PathBuf::from("/run/lock").join(format!("{}.lock", reg.name)))?;
    if !service.executable.is_file() || !service.directory.is_dir() {
        bail!("the executable and service data directory must exist before installation");
    }
    super::linux_directory::prepare(service)?;
    let content = definition(service, &reg, version)?;
    transaction::run(
        &reg.file.with_extension("transaction"),
        &reg.marker,
        || capture(&reg),
        |before| {
            let changed = before.unit.as_deref() != Some(content.as_bytes())
                || before.executable.as_deref() != Some(service.executable.as_os_str().as_bytes());
            if changed {
                if before.unit.is_some() {
                    systemctl(&["disable", &reg.name])?;
                    reg.watchdog.stop()?;
                    systemctl(&["stop", &reg.name])?;
                }
                publish_link(&reg.executable, &service.executable)?;
                publish(&reg.file, content.as_bytes(), Permissions::KeepExisting)?;
            }
            reg.watchdog.write(&reg.name, version)?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", &reg.name])?;
            systemctl(&["start", &reg.name])?;
            reg.watchdog.start()?;
            // Its user restarts it on a new release without a password: the
            // node's own, or whoever installed a hub with sudo. Without the
            // rule, a renewal waits for an administrator.
            let served = match service.kind {
                super::Kind::Node => Some(service.uid),
                super::Kind::Hub => std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok()),
            };
            if let Some(uid) = served
                && let Err(error) = super::linux_restart::allow(&reg.id, uid)
            {
                eprintln!(
                    "fastctx: automatic service restart was not authorized: {error:#}; a renewal will need an administrator"
                );
            }
            reg.status(true)
        },
        |before| restore(&reg, before),
    )
}

/// The running manager's version, not an upgraded client's: the unit is
/// written for what it reads (`definition`).
fn manager_version() -> anyhow::Result<u32> {
    let output = checked(Command::new("busctl").args([
        "--system",
        "get-property",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "Version",
    ]))?;
    let value = busctl_value(&output)?;
    let text = value.as_str().context("systemd gave its Version as no string")?;
    text.trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .with_context(|| format!("unrecognized systemd version {text:?}"))
}

/// A property as busctl prints it plainly: its type, then its value, as
/// `s "active"` or `i 0`.
fn busctl_value(output: &str) -> anyhow::Result<serde_json::Value> {
    let (kind, value) = output.trim().split_once(' ').context("busctl printed no value")?;
    match kind {
        "s" | "o" => {
            let inner = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .context("busctl printed an unquoted string")?;
            Ok(serde_json::Value::String(inner.replace("\\\"", "\"").replace("\\\\", "\\")))
        }
        "b" => Ok(serde_json::Value::Bool(value == "true")),
        _ => Ok(serde_json::Value::from(value.parse::<i64>().with_context(|| format!("busctl printed {output:?}"))?)),
    }
}

fn capture(reg: &Registration) -> anyhow::Result<Before> {
    let unit = reg.read()?;
    let executable = read_link(&reg.executable)?.map(|path| path.into_os_string().into_vec());
    if unit.is_none() && executable.is_some() {
        bail!("{} exists without its owned unit", reg.executable.display());
    }
    let status = reg.status(unit.is_some())?;
    Ok(Before { unit, executable, status, watchdog: reg.watchdog.capture()? })
}

fn restore(reg: &Registration, before: &Before) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    let mut record = |result: anyhow::Result<()>| {
        if let Err(error) = result {
            errors.push(format!("{error:#}"));
        }
    };
    record((|| {
        if reg.file.try_exists()? {
            systemctl(&["disable", &reg.name])?;
        }
        reg.watchdog.stop()
    })());
    record((|| {
        // is-active reads ActiveState alone: LSB 3 means inactive and 4
        // means absent. Unlike show it never serializes argv.
        let output = Command::new("systemctl").args(["--system", "is-active", &reg.name]).output()?;
        let state = std::str::from_utf8(&output.stdout)?.trim();
        if !matches!(output.status.code(), Some(0 | 3 | 4)) {
            bail!("systemctl is-active exited {}: {}", output.status, String::from_utf8_lossy(&output.stderr));
        }
        if !matches!(state, "inactive" | "failed" | "unknown") {
            systemctl(&["stop", &reg.name])?;
        }
        Ok(())
    })());
    // A missing file here was never enabled, or was already disabled by
    // uninstall before deletion. systemctl disable rejects absent files.
    record((|| {
        if reg.file.try_exists()? {
            systemctl(&["disable", &reg.name])?;
        }
        Ok(())
    })());
    record(match before.unit.as_deref() {
        Some(bytes) => publish(&reg.file, bytes, Permissions::KeepExisting).map_err(Into::into),
        None => match fs::remove_file(&reg.file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    });
    record(match before.executable.as_deref() {
        Some(target) => publish_link(&reg.executable, std::path::Path::new(std::ffi::OsStr::from_bytes(target))),
        None => delete_file(&reg.executable).map_err(Into::into),
    });
    record(systemctl(&["daemon-reload"]).map(|_| ()));
    if before.status.enabled {
        record(systemctl(&["enable", &reg.name]).map(|_| ()));
    }
    if ["active/", "activating/", "reloading/"].iter().any(|state| before.status.state.starts_with(state)) {
        record(systemctl(&["start", &reg.name]).map(|_| ()));
    }
    record(reg.watchdog.restore(&before.watchdog));
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

pub(super) fn uninstall(service: &Service) -> anyhow::Result<Status> {
    let reg = Registration::new(service);
    let _lock = FileLock::acquire(&PathBuf::from("/run/lock").join(format!("{}.lock", reg.name)))?;
    transaction::run(
        &reg.file.with_extension("transaction"),
        &reg.marker,
        || capture(&reg),
        |before| {
            if before.unit.is_some() {
                systemctl(&["disable", &reg.name])?;
                reg.watchdog.stop()?;
                systemctl(&["stop", &reg.name])?;
                fs::remove_file(&reg.file)?;
                delete_file(&reg.executable)?;
            }
            super::linux_restart::forbid(&reg.id)?;
            reg.watchdog.delete()?;
            systemctl(&["daemon-reload"])?;
            reg.status(false)
        },
        |before| restore(&reg, before),
    )
}

pub(super) fn systemctl(args: &[&str]) -> anyhow::Result<String> {
    checked(Command::new("systemctl").arg("--system").args(args))
}

/// The unit, written for the systemd running: 249 brought `OnSuccess=`,
/// which restores an explicit stop at once; 242 the `:` prefix preserving
/// `$`; 240 `Type=exec`; 230 the start limit in `[Unit]`. An older one
/// gets their older forms, and an explicit stop is restored by the timer's
/// check within a minute (`linux_watchdog`).
fn definition(service: &Service, reg: &Registration, version: u32) -> anyhow::Result<String> {
    // The node owns its ordinary children; independently supervised world
    // jobs must survive a foreground restart or an idle identity handoff.
    let args = std::iter::once(reg.executable.as_os_str())
        .chain(service.arguments().iter().map(|arg| arg.as_os_str()))
        .map(word)
        .collect::<Vec<_>>()
        .join(" ");
    let (start, args) = if version >= 249 { (":", args) } else { ("", args.replace("\\x24", "$$")) };
    let mut unit = format!("{}[Unit]\nDescription=fastctx {}\n", reg.marker, service.role());
    if version >= 230 {
        unit.push_str("StartLimitIntervalSec=0\n");
    }
    if version >= 249 {
        unit.push_str(&format!("OnSuccess={}\n", reg.watchdog.checker));
    }
    unit.push_str(&format!("\n[Service]\nType={}\n", if version >= 240 { "exec" } else { "simple" }));
    if version < 230 {
        unit.push_str("StartLimitInterval=0\n");
    }
    unit.push_str(&format!(
        "User={}\nExecStart={start}{args}\nRestart=always\nRestartSec=0\nKillMode=process\nStandardInput=null\nStandardOutput=journal\nStandardError=journal\n\n[Install]\nWantedBy=multi-user.target\n",
        service.uid
    ));
    Ok(unit)
}

fn read_link(path: &std::path::Path) -> anyhow::Result<Option<PathBuf>> {
    match fs::read_link(path) {
        Ok(target) => Ok(Some(target)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading service executable link {}", path.display())),
    }
}

fn delete_file(path: &std::path::Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// systemd rejects control characters in an executable name even when they
/// were correctly C-escaped. A stable symlink lets the kernel resolve the
/// original native pathname while systemd still execs the service directly.
fn publish_link(path: &std::path::Path, target: &std::path::Path) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        ".fastctx-service-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::os::unix::fs::symlink(target, &temporary)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let cleanup = fs::remove_file(&temporary);
        return match cleanup {
            Ok(()) => Err(error.into()),
            Err(cleanup) => Err(error).context(format!("removing staged executable link also failed: {cleanup}")),
        };
    }
    fs::File::open(path.parent().context("service link has no parent")?)?.sync_all()?;
    Ok(())
}

/// systemd unquotes C escapes and then expands percent specifiers. The ':'
/// ExecStart prefix disables dollar substitution (including in argv[0]).
/// Byte escapes preserve non-UTF-8 names, newlines and every other filename byte.
fn word(value: &std::ffi::OsStr) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut result = String::from("\"");
    for byte in value.as_bytes() {
        match byte {
            b'%' => result.push_str("%%"),
            b'/' | b'.' | b'-' | b'_' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' => result.push(*byte as char),
            byte => result.push_str(&format!("\\x{byte:02x}")),
        }
    }
    result.push('"');
    result
}
