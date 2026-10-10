//! An image-only guardian update preserves SCM registration and recovery at
//! every step. Its caller holds the user's startup gate and has verified a
//! serving bridge outside the guardian's job. No service is ever disabled.

use anyhow::{Context as _, ensure};
use fastctx_platform::{FileLock, Permissions, publish};
use serde::{Deserialize, Serialize};
use windows_service::service::{ServiceAccess, ServiceState};

use super::{config::Config, owner, scm};
use crate::renewal as images;
use crate::service::Service;

#[derive(Serialize, Deserialize)]
struct Running {
    process: fastctx_types::ProcessIdentity,
    digest: String,
}

pub(super) fn record(config: &Config) -> anyhow::Result<()> {
    let running = Running { process: fastctx_platform::current_identity()?, digest: images::build()?.digest };
    publish(&config.root()?.join("running.json"), &serde_json::to_vec(&running)?, Permissions::KeepExisting)?;
    Ok(())
}

pub(crate) fn current(service: &Service, digest: &str) -> anyhow::Result<bool> {
    let config = Config::new(service)?;
    let root = config.root()?;
    let running: Option<Running> = images::read_json(&root.join("running.json"))?;
    Ok(match running {
        Some(running) => {
            running.digest == digest
                && fastctx_platform::is_alive(&running.process)?
                && images::digest(&root.join("guardian.exe"))? == digest
        }
        None => false,
    })
}

pub(crate) fn staged(service: &Service, digest: &str) -> anyhow::Result<bool> {
    let config = Config::new(service)?;
    Ok(images::digest(&config.root()?.join("guardian.exe"))? == digest)
}

/// Stages the approved image in a node's or a hub's protected registration.
/// A node's is restarted later behind its replacement bridge; a hub's has no
/// bridge, and its caller restarts it at once.
pub(crate) fn replace(service: &Service, digest: &str) -> anyhow::Result<()> {
    let expected = Config::new(service)?;
    let config = Config::read(&expected.name)?;
    ensure!(
        config.executable() == service.executable && config.directory() == service.directory,
        "registered guardian paths differ from the replacement"
    );
    let _owner = owner::AdministratorOwner::enter()?;
    let root = config.root()?;
    let _lock = FileLock::acquire(&root.join("manager.lock"))?;
    super::transaction::require_complete(&root.join("transaction.json"))?;
    ensure!(scm::capture(&config.name)?.is_some(), "guardian registration is absent");
    ensure!(images::build()?.digest == digest, "approved guardian image differs");
    images::publish_image(&std::env::current_exe()?, &root.join("guardian.exe"))?;
    Ok(())
}

pub(crate) fn restart(service: &Service, digest: &str) -> anyhow::Result<()> {
    let config = Config::new(service)?;
    ensure!(staged(service, digest)?, "the approved guardian image is not in place");
    let registered = scm::open(&config.name, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START)?
        .context("guardian registration disappeared")?;
    // SCM's existing failure action and independent check remain enabled.
    // A crash here cannot leave boot startup disabled or require another UAC.
    let mut requested = false;
    let mut stopped = None;
    loop {
        let status = registered.query_status()?;
        if !requested {
            match status.current_state {
                ServiceState::Running => {
                    stopped = status.process_id.map(fastctx_platform::identity_of).transpose()?.flatten();
                    registered.stop()?;
                    requested = true;
                    continue;
                }
                ServiceState::Stopped => {
                    registered.start::<&str>(&[])?;
                    requested = true;
                    continue;
                }
                ServiceState::StopPending => {
                    stopped = status.process_id.map(fastctx_platform::identity_of).transpose()?.flatten();
                    requested = true;
                }
                _ => {}
            }
        }
        if requested && status.current_state == ServiceState::Running && current(service, digest)? {
            let running: Option<Running> = images::read_json(&config.root()?.join("running.json"))?;
            if running.is_some_and(|running| Some(running.process) != stopped) {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
