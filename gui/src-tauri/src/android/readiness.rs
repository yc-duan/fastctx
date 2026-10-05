//! A picker may deliver its result before the Activity resumes. Do not send
//! that upload into the connection the pending foreground resume will replace.

use fastctx_client::{World, device::Device};
use fastctx_net::link::Status;
use fastctx_types::world::LinkState;
use std::sync::Mutex;

static REPLACING: Mutex<Option<jiff::Timestamp>> = Mutex::new(None);

pub(super) fn reconnecting(device: &Device) {
    *REPLACING.lock().expect("Android connection readiness") = match device.state().link {
        LinkState::Online { since, .. } => Some(since),
        _ => None,
    };
}

pub(crate) async fn wait(world: &World) -> Result<(), String> {
    let mut visible = super::visibility();
    let mut connection =
        world.device().expect("Android device").connection_changes().map_err(|error| error.to_string())?;
    loop {
        let shown = *visible.borrow_and_update();
        let state = connection.borrow_and_update().clone();
        match state {
            Status::Online { since, .. }
                if shown && Some(since) != *REPLACING.lock().expect("Android connection readiness") =>
            {
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
