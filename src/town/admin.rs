//! What the town answers on the node's local admin channel: requests from `fastctx` commands
//! and the TUI on this machine.

use super::core::{Spoken, Town};
use super::talk::TalkEvent;
use crate::runtime::local_ipc::BoxedStream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum TownRequest {
    /// Moves a resident into a town and answers with the town's map.
    Enter { resident: String, town: String },
    /// The user's own words for a resident, typed on this machine.
    Say { resident: String, text: String },
    /// Streams a resident's history and then its events, one frame each, until the watcher
    /// closes the connection.
    Watch { resident: String },
    /// A command ended; its record is at `record`. Sent by the run engine's end hook.
    JobEnded { record: std::path::PathBuf },
    /// Opens a resident for the user and puts the user's first words in.
    Open { agent: String, town: Option<String>, words: String },
    /// Stops the turn a resident is running, at the user's word.
    Interrupt { resident: String },
}

impl TownRequest {
    pub(crate) fn streams(&self) -> bool {
        matches!(self, Self::Watch { .. })
    }
}

pub(crate) async fn handle(town: &Town, request: TownRequest) -> Result<Value, String> {
    match request {
        TownRequest::Enter { resident, town: name } => Ok(json!({ "map": town.enter(&resident, &name)? })),
        TownRequest::Say { resident, text } => {
            let (spoken, turn) = town.say_as_user(&resident, &text).await?;
            let how = match spoken {
                Spoken::NewTurn => "started a new turn",
                Spoken::JoinedTurn => "joined its current turn",
                Spoken::Queued => "handed to its host",
            };
            Ok(json!({ "landed": how, "turn": turn }))
        }
        TownRequest::Watch { .. } => Err("A watch streams; it is not answered once.".to_string()),
        TownRequest::Open { agent, town: place, words } => Ok(json!({ "resident": town.open_resident(&agent, place, &words).await? })),
        TownRequest::Interrupt { resident } => Ok(json!({ "stopped": town.interrupt(&resident).await? })),
        TownRequest::JobEnded { record } => {
            town.job_ended(&record).await?;
            Ok(Value::Null)
        }
    }
}

/// Serves a watch on its own connection: every frame is one `TalkEvent`. Ends when the
/// watcher goes away or the resident's events end.
pub(crate) async fn stream(town: &Town, request: TownRequest, mut connection: BoxedStream) {
    let TownRequest::Watch { resident } = request else { return };
    let (history, mut events) = match town.watch(&resident).await {
        Ok(watch) => watch,
        Err(reason) => {
            let _ = send(&mut connection, &TalkEvent::Gone { reason }).await;
            return;
        }
    };
    for event in history.iter().chain([&TalkEvent::Live]) {
        if send(&mut connection, event).await.is_err() {
            return;
        }
    }
    let mut seen = super::talk::HistoryKeys::of(&history);
    while let Some(event) = events.recv().await {
        if seen.repeats(&event) {
            continue;
        }
        if send(&mut connection, &event).await.is_err() {
            return;
        }
    }
}

async fn send(connection: &mut BoxedStream, event: &TalkEvent) -> Result<(), String> {
    let bytes = serde_json::to_vec(event).map_err(|error| error.to_string())?;
    crate::world::node::admin::write_frame(connection, &bytes).await
}
