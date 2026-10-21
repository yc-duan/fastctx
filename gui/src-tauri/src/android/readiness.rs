//! A picker may deliver its result before the Activity resumes. Do not send
//! that upload into a connection the foreground resume is still asking
//! about: once the hub answers, it goes on that one; when the hub does not,
//! on the one that replaces it. The shared input's watch opens the same way.

use fastctx_client::{World, device::Device};
use fastctx_net::link::Status;
use fastctx_types::world::LinkState;
use fastctx_types::{HubName, access::Written};
use std::sync::Mutex;

/// Each hub's connection as it stood when a resume began asking whether it
/// holds: when it came up, and how many checks the hub had answered then.
static ASKED: Mutex<Vec<(HubName, jiff::Timestamp, u64)>> = Mutex::new(Vec::new());

pub(super) fn reconnecting(device: &Device) {
    let answered = device.checks();
    *ASKED.lock().expect("Android connection readiness") = device
        .state()
        .hubs
        .into_iter()
        .filter_map(|hub| match hub.link {
            LinkState::Online { since, .. } => {
                let checks = answered.iter().find(|(name, _)| *name == hub.hub).map_or(0, |(_, checks)| *checks);
                Some((hub.hub, since, checks))
            }
            _ => None,
        })
        .collect();
}

pub(crate) async fn wait(world: &World, machine: Option<&Written>) -> Result<(), String> {
    let mut visible = super::visibility();
    let device = world.device().expect("Android device");
    let (hub, mut connection) = device.machine_changes(machine).map_err(|error| error.to_string())?;
    let mut checks = device.machine_checks(machine).map_err(|error| error.to_string())?;
    // The connection a resume found, with no check answered on it since.
    let asking = |since: jiff::Timestamp, answered: u64| {
        ASKED
            .lock()
            .expect("Android connection readiness")
            .iter()
            .any(|(asked, at, checks)| *asked == hub && *at == since && *checks == answered)
    };
    loop {
        let shown = *visible.borrow_and_update();
        let state = connection.borrow_and_update().clone();
        let answered = *checks.borrow_and_update();
        match state {
            Status::Online { since, .. } if shown && !asking(since, answered) => {
                return Ok(());
            }
            Status::Stopped { reason } => return Err(reason),
            _ => (),
        }
        tokio::select! {
            changed = visible.changed() => changed.map_err(|error| error.to_string())?,
            changed = connection.changed() => changed.map_err(|error| error.to_string())?,
            changed = checks.changed() => changed.map_err(|error| error.to_string())?,
        }
    }
}
