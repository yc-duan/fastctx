//! A SYSTEM guardian owns only process lifetime and token selection. Nodes
//! run under their user's token, and communicate through inherited pipes.

mod account;
mod adopt;
mod child;
mod config;
mod firewall;
pub mod managed;
mod host;
mod owner;
mod policy;
mod renewal;
mod runtime;
mod scm;
mod security;
mod system_cache;
mod task;

pub(super) fn registrations(installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<super::Registration>> {
    let mut found = Vec::new();
    let executable = installation.root.join("bin/fastctx.exe");
    let mut node = super::Service::new(super::Kind::Node, &executable, &installation.root);
    node.user_sid = Some(installation.user_sid.clone());
    let mut candidates = vec![node];
    let mut hubs = super::registered_hubs(&installation.root)?;
    if installation.root.join("hub/hub.json").try_exists()? {
        hubs.push(installation.root.join("hub"));
    }
    hubs.sort();
    hubs.dedup();
    candidates.extend(hubs.into_iter().map(|directory| super::Service::new(super::Kind::Hub, &executable, directory)));
    for service in candidates {
        let account = service.user_sid.as_deref().unwrap_or("S-1-5-18");
        let name = service.id(account.as_bytes());
        let directory = security::directory(&name)?;
        let installed = scm::capture(&name)?.is_some();
        let protected = directory.try_exists()?;
        if !installed && !protected {
            continue;
        }
        if service.kind == super::Kind::Node && (installed || directory.join("config.json").try_exists()?) {
            let config = Config::read(&name)?;
            if config.executable() != executable || config.directory() != installation.root {
                bail!("the node registration has different ownership coordinates");
            }
        }
        found.push(super::Registration {
            name,
            kind: service.kind,
            executable: service.executable,
            directory: service.directory,
            artifacts: vec![directory],
        });
    }
    Ok(found)
}

use std::fs;
use std::path::Path;

use anyhow::{Context as _, bail};
use fastctx_platform::FileLock;
use fastctx_platform::atomic::{Permissions, publish};
use serde::{Deserialize, Serialize};
use windows_service::service::{ServiceAccess, ServiceExitCode, ServiceStartType, ServiceState, UserEventCode};

use super::{Service, Status, transaction};
use config::Config;

pub(super) use adopt::ask as adopt_guardian;
pub(super) use renewal::{
    current as guardian_current, replace as renew_guardian, restart as restart_guardian, staged as guardian_staged,
};
pub(super) use system_cache::retire as retire_system_cache;

#[derive(Serialize, Deserialize)]
struct Before {
    registration: Option<scm::Snapshot>,
    task: task::Snapshot,
    config: Option<Vec<u8>>,
    image: Option<Vec<u8>>,
    payload: Option<Vec<u8>>,
    batch_right: Option<bool>,
    /// A hub's inbound firewall rule was there.
    #[serde(default)]
    firewall: bool,
}

pub(super) fn install(service: &Service) -> anyhow::Result<Status> {
    let config = Config::new(service)?;
    let _owner = owner::AdministratorOwner::enter()?;
    let root = security::prepare(&config.name, config.target.as_ref().map(|target| target.sid.as_str()))?;
    // Account rights are shared by all registrations. Serialize their
    // transactions so one failed install cannot undo another's committed grant.
    let _policy_lock = FileLock::acquire(&root.parent().context("guardian parent is absent")?.join("policy.lock"))?;
    let _lock = FileLock::acquire(&root.join("manager.lock"))?;
    if !service.executable.is_file() || !service.directory.is_dir() {
        bail!("the node executable and data directory must exist");
    }
    let checker = task::Task::new(&config.name);
    let image = fs::read(std::env::current_exe()?)?;
    let payload = (config.kind == super::Kind::Hub).then(|| fs::read(&service.executable)).transpose()?;
    let bytes = serde_json::to_vec(&config)?;
    transaction::run(
        &root.join("transaction.json"),
        &config.name,
        || capture(&config, &checker),
        |before| {
            if let Some(target) = &config.target {
                policy::set_batch(&target.sid, true).context("granting batch logon to the node account")?;
            }
            let changed = before.image.as_deref() != Some(&image)
                || before.config.as_deref() != Some(&bytes)
                || before.payload != payload;
            if changed {
                // Disabling SCM precedes the checker so a queued check cannot
                // revive a process while its image is being replaced.
                scm::stop(&config.name)?;
                checker.delete()?;
                publish(&root.join("guardian.exe"), &image, Permissions::KeepExisting)?;
                if let Some(payload) = &payload {
                    publish(&root.join("payload.exe"), payload, Permissions::KeepExisting)?;
                }
                publish(&root.join("config.json"), &bytes, Permissions::KeepExisting)?;
            }
            if config.kind == super::Kind::Hub {
                firewall::allow(&config.name, &root.join("payload.exe"))?;
            }
            if before.registration.is_none() {
                scm::create(&config.name, config.target.as_ref().map(|target| target.sid.as_str()))?;
            }
            checker.install(&config.name, config.target.as_ref().map(|target| target.sid.as_str()))?;
            scm::enable(&config.name)?;
            scm::start(&config.name)?;
            inspect(&config)
        },
        |before| restore(&config, &checker, before),
    )
}

/// Stops the guardian and keeps it stopped: disabled, with no restart on
/// failure, which its scheduled check leaves as it is.
pub(super) fn hold(service: &Service) -> anyhow::Result<Status> {
    let config = Config::new(service)?;
    let _owner = owner::AdministratorOwner::enter()?;
    scm::stop(&config.name)?;
    inspect(&config)
}

/// Starts it again at boot and after a failure, and now.
pub(super) fn release(service: &Service) -> anyhow::Result<Status> {
    let config = Config::new(service)?;
    let _owner = owner::AdministratorOwner::enter()?;
    scm::enable(&config.name)?;
    scm::start(&config.name)?;
    inspect(&config)
}

pub(super) fn uninstall(service: &Service) -> anyhow::Result<Status> {
    let config = Config::new(service)?;
    let _owner = owner::AdministratorOwner::enter()?;
    let root = config.root()?;
    if !root.try_exists()? {
        if scm::capture(&config.name)?.is_some() {
            bail!("guardian registration has lost its protected configuration");
        }
        return inspect(&config);
    }
    security::prepare(&config.name, config.target.as_ref().map(|target| target.sid.as_str()))?;
    let _policy_lock = FileLock::acquire(&root.parent().context("guardian parent is absent")?.join("policy.lock"))?;
    let _lock = FileLock::acquire(&root.join("manager.lock"))?;
    let checker = task::Task::new(&config.name);
    transaction::run(
        &root.join("transaction.json"),
        &config.name,
        || capture(&config, &checker),
        |_| {
            scm::stop(&config.name)?;
            checker.delete()?;
            scm::delete(&config.name)?;
            if config.kind == super::Kind::Hub {
                firewall::remove(&config.name)?;
            }
            delete(&root.join("guardian.exe"))?;
            delete(&root.join("payload.exe"))?;
            delete(&root.join("config.json"))?;
            inspect(&config)
        },
        |before| restore(&config, &checker, before),
    )
}

pub(super) fn definition_current(service: &Service) -> anyhow::Result<bool> {
    let config = Config::new(service)?;
    let path = config.root()?.join("config.json");
    let Some(actual) = crate::renewal::read_json::<Config>(&path)? else { return Ok(false) };
    Ok(serde_json::to_value(actual)? == serde_json::to_value(config)?)
}

pub(super) fn status(service: &Service) -> anyhow::Result<Status> {
    let config = Config::new(service)?;
    transaction::require_complete(&config.root()?.join("transaction.json"))?;
    inspect(&config)
}

fn inspect(config: &Config) -> anyhow::Result<Status> {
    let service = scm::open(&config.name, ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS)?;
    let (installed, enabled, state, last_exit) = match service {
        None => (false, false, "absent".into(), None),
        Some(service) => {
            scm::capture(&config.name)?;
            let status = service.query_status()?;
            let exit = match status.exit_code {
                ServiceExitCode::Win32(code) | ServiceExitCode::ServiceSpecific(code) => i64::from(code),
            };
            (
                true,
                service.query_config()?.start_type == ServiceStartType::AutoStart,
                format!("{:?}", status.current_state).to_ascii_lowercase(),
                Some(exit),
            )
        }
    };
    Ok(Status {
        name: config.name.clone(),
        installed,
        enabled,
        state,
        last_exit,
        logs: config.root()?.join("guardian.log").display().to_string(),
        recovery: Some(task::Task::new(&config.name).status()?),
    })
}

fn capture(config: &Config, checker: &task::Task) -> anyhow::Result<Before> {
    let root = config.root()?;
    let registration = scm::capture(&config.name)?;
    let bytes = read(&root.join("config.json"))?;
    if bytes.is_some() {
        Config::read(&config.name)?;
    }
    if registration.is_some() && bytes.is_none() {
        bail!("guardian service exists without its owned configuration");
    }
    Ok(Before {
        registration,
        task: checker.capture()?,
        config: bytes,
        image: read(&root.join("guardian.exe"))?,
        payload: read(&root.join("payload.exe"))?,
        batch_right: config.target.as_ref().map(|target| policy::batch_granted(&target.sid)).transpose()?,
        firewall: config.kind == super::Kind::Hub && firewall::present(&config.name)?,
    })
}

fn restore(config: &Config, checker: &task::Task, before: &Before) -> anyhow::Result<()> {
    let root = config.root()?;
    let mut errors = Vec::new();
    let mut record = |result: anyhow::Result<()>| {
        if let Err(error) = result {
            errors.push(format!("{error:#}"));
        }
    };
    record(scm::stop(&config.name));
    record(checker.delete());
    for (file, bytes) in
        [("guardian.exe", &before.image), ("payload.exe", &before.payload), ("config.json", &before.config)]
    {
        record(match bytes {
            Some(bytes) => publish(&root.join(file), bytes, Permissions::KeepExisting).map_err(Into::into),
            None => delete(&root.join(file)).map_err(Into::into),
        });
    }
    record(scm::restore(
        &config.name,
        before.registration.as_ref(),
        config.target.as_ref().map(|target| target.sid.as_str()),
    ));
    record(checker.restore(&before.task));
    if let (Some(target), Some(granted)) = (&config.target, before.batch_right) {
        record(policy::set_batch(&target.sid, granted));
    }
    if config.kind == super::Kind::Hub {
        record(if before.firewall {
            firewall::allow(&config.name, &root.join("payload.exe"))
        } else {
            firewall::remove(&config.name)
        });
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

fn read(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn delete(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn run_host(name: &str) -> anyhow::Result<()> {
    host::dispatch(name)
}

pub(super) fn check(name: &str) -> anyhow::Result<()> {
    security::validate_name(name)?;
    Config::read(name)?;
    let Some(service) = scm::open(
        name,
        ServiceAccess::QUERY_CONFIG
            | ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::USER_DEFINED_CONTROL,
    )?
    else {
        return Ok(());
    };
    // A disabled service is being removed or deliberately disabled. Checks
    // cannot undo that persisted management decision.
    if service.query_config()?.start_type == ServiceStartType::Disabled {
        return Ok(());
    }
    match service.query_status()?.current_state {
        ServiceState::Stopped => scm::start(name)?,
        ServiceState::Running => {
            service.notify(UserEventCode::from_raw(128).context("guardian check control")?)?;
        }
        _ => {}
    }
    Ok(())
}
