//! A picker may deliver its result before the Activity resumes. Do not send
//! that upload into the connection the pending foreground resume will replace.

use fastctx_client::{World, device::Device};
use fastctx_net::link::Status;
use fastctx_types::world::LinkState;
use fastctx_types::{HubName, MachineName};
use std::sync::Mutex;

/// Each hub's connection as it stood when a resume began replacing it.
static REPLACING: Mutex<Vec<(HubName, jiff::Timestamp)>> = Mutex::new(Vec::new());

pub(super) fn reconnecting(device: &Device) {
    *REPLACING.lock().expect("Android connection readiness") = device
        .state()
        .hubs
        .into_iter()
        .filter_map(|hub| match hub.link {
            LinkState::Online { since, .. } => Some((hub.hub, since)),
            _ => None,
        })
        .collect();
}

pub(crate) async fn wait(world: &World, machine: Option<&MachineName>) -> Result<(), String> {
    let mut visible = super::visibility();
    let (hub, mut connection) =
        world.device().expect("Android device").machine_changes(machine).map_err(|error| error.to_string())?;
    let replacing = |since: jiff::Timestamp| {
        REPLACING.lock().expect("Android connection readiness").iter().any(|(replaced, at)| *replaced == hub && *at == since)
    };
    loop {
        let shown = *visible.borrow_and_update();
        let state = connection.borrow_and_update().clone();
        match state {
            Status::Online { since, .. } if shown && !replacing(since) => {
                return Ok(());
            }
            Status::Stopped { reason } => return Err(reason),
            _ => (),
        }
        tokio::select! {
            changed = visible.changed() => changed.map_err(|error| error.to_string())?,
            changed = connection.changed() => changed.map_err(|error| error.to_string())?,
        }
    }
}
