//! An image-only guardian update preserves SCM registration and recovery at
//! every step. Its caller holds the user's startup gate and has verified a
//! serving bridge outside the guardian's job. No service is ever disabled.

use anyhow::{Context as _, ensure};
use fastctx_platform::{FileLock, Permissions, publish};
use serde::{Deserialize, Serialize};
use windows_service::service::{ServiceAccess, ServiceState};

use super::{config::Config, owner, scm};
use crate::renewal as images;
use crate::service::{Kind, Service};

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
            running.digest == digest && fastctx_platform::is_alive(&running.process)? && staged(service, digest)?
        }
        None => false,
    })
}

/// Whether the image `digest` is in place: the guardian's, and a hub's
/// program beside it, which the guardian runs.
pub(crate) fn staged(service: &Service, digest: &str) -> anyhow::Result<bool> {
    let config = Config::new(service)?;
    let root = config.root()?;
    Ok(images::digest(&root.join("guardian.exe"))? == digest
        && (config.kind != Kind::Hub || images::digest(&root.join("payload.exe"))? == digest))
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
    if config.kind == Kind::Hub {
        images::publish_image(&std::env::current_exe()?, &root.join("payload.exe"))?;
    }
    Ok(())
}

/// How long the guardian is given to stop and come back on the new image:
/// it takes seconds; one held in a pending state or failing to start is
/// taken for stuck, its gates let go, and the renewal or repair tried again.
const RESTARTING: std::time::Duration = std::time::Duration::from_secs(10 * 60);

pub(crate) fn restart(service: &Service, digest: &str) -> anyhow::Result<()> {
    let config = Config::new(service)?;
    ensure!(staged(service, digest)?, "the approved guardian image is not in place");
    let registered = scm::open(
        &config.name,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START | ServiceAccess::USER_DEFINED_CONTROL,
    )?
    .context("guardian registration disappeared")?;
    // SCM's existing failure action and independent check remain enabled.
    // A crash here cannot leave boot startup disabled or require another UAC.
    let mut requested = false;
    let mut started = false;
    let mut stopped = None;
    let began = std::time::Instant::now();
    loop {
        let status = registered.query_status()?;
        ensure!(
            began.elapsed() < RESTARTING,
            "the guardian did not come back on the new program within {} minutes; it is {:?}",
            RESTARTING.as_secs() / 60,
            status.current_state
        );
        if !requested {
            match status.current_state {
                ServiceState::Running => {
                    stopped = status.process_id.map(fastctx_platform::identity_of).transpose()?.flatten();
                    super::adopt::planned(&registered);
                    registered.stop()?;
                    requested = true;
                    continue;
                }
                ServiceState::Stopped => {
                    start(&registered)?;
                    (requested, started) = (true, true);
                    continue;
                }
                ServiceState::StopPending => {
                    stopped = status.process_id.map(fastctx_platform::identity_of).transpose()?.flatten();
                    requested = true;
                }
                _ => {}
            }
        }
        // A planned stop ends well, so the service manager does not start
        // the guardian again as after a failure: it is started here.
        if requested && !started && status.current_state == ServiceState::Stopped {
            start(&registered)?;
            started = true;
            continue;
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

/// Starts the guardian, unless its independent check started it first.
fn start(registered: &windows_service::service::Service) -> anyhow::Result<()> {
    match registered.start::<&str>(&[]) {
        Err(windows_service::Error::Winapi(error))
            if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING as i32) =>
        {
            Ok(())
        }
        started => started.map_err(Into::into),
    }
}
