//! How this computer's Fastctx is doing, for its machine page: its health,
//! the rules it broke and the scenes it left (`Call::DoctorReadings`), asked
//! of the node here alone.

use fastctx_types::health::Health;
use fastctx_types::readings::Readings;
use fastctx_types::{Call, Reply};
use serde::Serialize;

use crate::Shell;

#[derive(Serialize)]
pub(crate) struct DoctorReadings {
    readings: Readings,
    health: Health,
    /// The scenes it left, oldest first.
    scenes: Vec<String>,
}

#[fastctx_gui_command::command]
pub(crate) async fn doctor_readings(shell: tauri::State<'_, Shell>) -> Result<DoctorReadings, String> {
    let world = shell.connection.local_world()?;
    match world.call(Call::DoctorReadings).await.map_err(|error| error.to_string())?.0 {
        Reply::DoctorReadings { readings, health, scenes } => {
            Ok(DoctorReadings { readings: *readings, health, scenes })
        }
        other => Err(format!("the node answered something other than its readings: {other:?}")),
    }
}
