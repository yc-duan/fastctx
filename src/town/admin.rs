//! What the town answers on the node's local admin channel: requests from `fastctx` commands
//! and the TUI on this machine. Requests about a resident reach it wherever it lives: this
//! machine's town answers for its own, and asks the resident's machine for the others.

use super::core::Town;
use super::remote::calls::outcome_error;
use super::remote::verbs::{OpenCall, SayCall, StopCall, landed};
use super::talk::TalkEvent;
use super::who::Query;
use crate::runtime::local_ipc::BoxedStream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

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
    /// Opens a resident for the user, on this machine or the one `node` names, and puts the
    /// user's first words in.
    Open {
        agent: String,
        town: Option<String>,
        words: String,
        #[serde(default)]
        node: Option<String>,
    },
    /// Stops the turn a resident is running, at the user's word.
    Interrupt { resident: String },
    /// `fastctx w`: this machine's answer and those of the other machines the query covers.
    Who { query: Query },
    /// Opens a project or writes the next version of its charter, for `resident` or, when
    /// none is named, for the user.
    Charter {
        town: String,
        change: super::ledger::CharterChange,
        opening: bool,
        #[serde(default)]
        resident: Option<String>,
    },
    /// Adds an entry to a journal.
    JournalAdd {
        draft: super::ledger::EntryDraft,
        #[serde(default)]
        resident: Option<String>,
    },
    /// Writes the next version of a journal's description.
    Describe {
        town: String,
        journal: String,
        text: String,
        #[serde(default)]
        resident: Option<String>,
    },
}

impl TownRequest {
    pub(crate) fn streams(&self) -> bool {
        matches!(self, Self::Watch { .. })
    }
}

pub(crate) async fn handle(town: &Arc<Town>, request: TownRequest) -> Result<Value, String> {
    match request {
        TownRequest::Enter { resident, town: name } => Ok(json!({ "map": town.enter(&resident, &name)? })),
        TownRequest::Say { resident, text } => {
            let machine = town.locate(&resident).await?;
            if machine == town.machine() {
                let (spoken, turn) = town.say_as_user(&resident, &text).await?;
                return Ok(json!({ "landed": landed(spoken), "turn": turn }));
            }
            let outcome = town.call_town(&machine, "talk_say", json!(SayCall { resident, text })).await;
            if outcome.status != "ok" {
                return Err(outcome_error(&outcome));
            }
            Ok(json!({ "landed": outcome.facts.get("landed"), "turn": outcome.facts.get("turn") }))
        }
        TownRequest::Watch { .. } => Err("A watch streams; it is not answered once.".to_string()),
        TownRequest::Open { agent, town: place, words, node } => match node.filter(|node| node != town.machine()) {
            None => Ok(json!({ "resident": town.open_resident(&agent, place, &words).await? })),
            Some(machine) => {
                let outcome = town.call_town(&machine, "talk_open", json!(OpenCall { agent, town: place, words })).await;
                if outcome.status != "ok" {
                    return Err(outcome_error(&outcome));
                }
                Ok(json!({ "resident": outcome.facts.get("resident"), "machine": machine }))
            }
        },
        TownRequest::Interrupt { resident } => {
            let machine = town.locate(&resident).await?;
            if machine == town.machine() {
                return Ok(json!({ "stopped": town.interrupt(&resident).await? }));
            }
            let outcome = town.call_town(&machine, "talk_stop", json!(StopCall { resident })).await;
            if outcome.status != "ok" {
                return Err(outcome_error(&outcome));
            }
            Ok(json!({ "stopped": outcome.facts.get("stopped") }))
        }
        TownRequest::JobEnded { record } => {
            town.job_ended(&record).await?;
            Ok(Value::Null)
        }
        TownRequest::Who { query } => {
            let (answers, silent) = town.gather(&query).await;
            Ok(json!({ "answers": answers, "silent": silent }))
        }
        TownRequest::Charter { town: name, change, opening, resident } => {
            let charter = town.write_charter(&name, resident.as_deref(), change, opening).await?;
            Ok(json!({ "charter": super::project::render_charter(&name, &charter), "version": charter.version }))
        }
        TownRequest::JournalAdd { draft, resident } => Ok(json!({ "entry": town.add_entry(resident.as_deref(), draft).await? })),
        TownRequest::Describe { town: name, journal, text, resident } => {
            Ok(json!({ "version": town.describe_journal(resident.as_deref(), &name, &journal, &text).await? }))
        }
    }
}

/// Serves a watch on its own connection: every frame is one `TalkEvent`. A resident of this
/// machine is watched here; one living elsewhere through a subscription to its machine. Ends
/// when the watcher goes away or the resident's events end.
pub(crate) async fn stream(town: &Arc<Town>, request: TownRequest, connection: BoxedStream) {
    let TownRequest::Watch { resident } = request else { return };
    let (mut reader, mut writer) = tokio::io::split(connection);
    // A watcher sends nothing after its request; reading returns only when it goes away.
    let mut gone = [0_u8; 1];
    let machine = match town.locate(&resident).await {
        Ok(machine) => machine,
        Err(reason) => {
            let _ = send(&mut writer, &TalkEvent::Gone { reason }).await;
            return;
        }
    };
    if machine != town.machine() {
        let mut watch = match town.watch_elsewhere(&machine, &resident).await {
            Ok(watch) => watch,
            Err(reason) => {
                let _ = send(&mut writer, &TalkEvent::Gone { reason: format!("{machine}: {reason}") }).await;
                return;
            }
        };
        loop {
            let event = tokio::select! {
                event = watch.events.recv() => event,
                _ = reader.read(&mut gone) => return,
            };
            let Some(event) = event else { return };
            let ended = matches!(event, TalkEvent::Gone { .. });
            if send(&mut writer, &event).await.is_err() || ended {
                return;
            }
        }
    }
    let (history, mut events) = match town.watch(&resident).await {
        Ok(watch) => watch,
        Err(reason) => {
            let _ = send(&mut writer, &TalkEvent::Gone { reason }).await;
            return;
        }
    };
    for event in history.iter().chain([&TalkEvent::Live]) {
        if send(&mut writer, event).await.is_err() {
            return;
        }
    }
    let seen = super::talk::HistoryKeys::of(&history);
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = reader.read(&mut gone) => return,
        };
        let Some(event) = event else { return };
        if seen.repeats(&event) {
            continue;
        }
        if send(&mut writer, &event).await.is_err() {
            return;
        }
    }
}

async fn send(writer: &mut tokio::io::WriteHalf<BoxedStream>, event: &TalkEvent) -> Result<(), String> {
    let bytes = serde_json::to_vec(event).map_err(|error| error.to_string())?;
    crate::world::node::admin::write_frame(writer, &bytes).await
}
