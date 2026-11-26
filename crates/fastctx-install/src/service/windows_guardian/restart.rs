//! A user asks its guardian to start its node anew (`fastctx service node
//! restart`), with no administrator: the target user may send the guardian
//! its own controls. The guardian ends the nodes it keeps and any node that
//! holds the World without being kept, as a renewal's bridge left serving,
//! then prepares one as the user's logon now asks.

use anyhow::Context as _;
use windows_service::service::{ServiceAccess, ServiceState, UserEventCode};

use super::{config::Config, scm};
use crate::service::Service;

/// The user control that asks the guardian to start the node anew.
pub(super) const RESTART: u32 = 131;

/// As the guardian's user: asks it to start the node anew. False when the
/// guardian is not running to be asked.
pub(crate) fn ask(service: &Service) -> anyhow::Result<bool> {
    let config = Config::new(service)?;
    let Some(registered) = scm::open(&config.name, ServiceAccess::QUERY_STATUS | ServiceAccess::USER_DEFINED_CONTROL)?
    else {
        return Ok(false);
    };
    if registered.query_status()?.current_state != ServiceState::Running {
        return Ok(false);
    }
    registered.notify(UserEventCode::from_raw(RESTART).context("the restart control")?)?;
    Ok(true)
}

/// In the guardian: ends what holds the World of the user it keeps without
/// being one of its nodes, which are gone already. The process its holder
/// record names while the World's lock is held, or else the one serving its
/// endpoint; the process ended, if any.
pub(super) fn end_holder(config: &Config) -> anyhow::Result<Option<u32>> {
    let Some(target) = &config.target else { return Ok(None) };
    let world = fastctx_types::layout::WorldDir::under(&config.directory());
    let recorded = || -> anyhow::Result<Option<fastctx_types::ProcessIdentity>> {
        if fastctx_platform::FileLock::try_acquire(&world.node_lock())?.is_some() {
            return Ok(None);
        }
        let Ok(bytes) = std::fs::read(world.node_holder()) else { return Ok(None) };
        let Ok(holder) = serde_json::from_slice::<fastctx_types::ProcessIdentity>(&bytes) else { return Ok(None) };
        Ok(fastctx_platform::is_alive(&holder)?.then_some(holder))
    };
    let holder = match recorded()? {
        Some(holder) => Some(holder),
        None => match fastctx_platform::ipc::Endpoint::for_world(world.path()) {
            Ok(endpoint) => fastctx_platform::ipc::pipe_server(&endpoint, &target.sid)?,
            Err(_) => None,
        },
    };
    let Some(holder) = holder else { return Ok(None) };
    fastctx_platform::terminate::terminate(&holder)?;
    Ok(Some(holder.pid))
}
