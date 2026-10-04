//! The files the World's mod and the node exchange for one Claude session,
//! in `<world>/claude/<session id>/` (the mod's side is `world.js`).
//!
//! The mod writes `session.json` when the session starts and one event file
//! per turn, tool use, hand-over and end, each followed by a `.done` marker
//! once it is whole, since `$.fs.write` promises nothing about being atomic.
//! The node writes what it has for the session into `inbox/`, each file
//! whole before it appears; the mod says in an event which turn took it,
//! and writes a receipt so that it never hands it over again. The node asks
//! for a running turn to be stopped in `stop.json`.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, TurnEnd, TurnHow};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the sessions' directories are.
pub fn sessions(world: &Path) -> PathBuf {
    world.join("claude")
}

/// What the mod says of its session when it starts.
#[derive(Clone, Debug, Deserialize)]
pub struct Registration {
    pub session: String,
    /// `FASTCTX_RESIDENT`, as `<id>@<machine>`, when the World started it.
    pub resident: Option<String>,
    /// The session's inbox address, which pairs it with its MCP connection.
    pub inbox_address: Option<String>,
    /// How the session was started: `cli`, `claude-vscode` and the like.
    pub entrypoint: Option<String>,
    pub model: Option<Value>,
    pub version: Option<Value>,
}

impl Registration {
    /// The model's name, as the session gave it.
    pub fn model_name(&self) -> Option<String> {
        match &self.model {
            Some(Value::String(model)) => Some(model.clone()),
            Some(Value::Object(model)) => model.get("id").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    }

    pub fn version_text(&self) -> Option<String> {
        match &self.version {
            Some(Value::String(version)) => Some(version.clone()),
            Some(Value::Object(version)) => version.get("version").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    }
}

/// The registration of the session in `directory`, once it is whole.
pub fn registration(directory: &Path) -> io::Result<Option<Registration>> {
    let path = directory.join("session.json");
    match std::fs::read(&path) {
        // Written by the mod in one go, but read while being written it can
        // be cut short; the next look reads it whole.
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// What the mod reported, in the World's terms.
#[derive(Debug)]
pub enum Reported {
    Happened(SessionEvent),
    /// The session ended; its mod stopped with it.
    Ended {
        reason: Option<String>,
    },
    /// The mod handed these inbox files to the model, in `turn`.
    Handed {
        ids: Vec<String>,
        turn: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum ModEvent {
    TurnStart { turn: Option<String>, at: Option<Timestamp> },
    TurnComplete { turn: Option<String>, at: Option<Timestamp>, aborted: bool, ms: Option<u64>, answer: Option<String> },
    ToolStart { turn: Option<String>, at: Option<Timestamp>, tool: Option<String>, detail: Option<String> },
    ToolEnd { turn: Option<String>, at: Option<Timestamp>, tool: Option<String>, detail: Option<String>, failed: bool },
    Trouble { at: Option<Timestamp>, file: Option<String>, text: Option<String> },
    SessionEnd { reason: Option<String> },
    StopFailed { at: Option<Timestamp>, turn: Option<String>, text: Option<String> },
    Handed { deliveries: Vec<String>, turn: Option<String> },
}

/// Takes every whole event the mod wrote, oldest first. An event that
/// cannot be read is moved to `damaged/` and said in the problems; the
/// newest without its marker is left, as it may still be being written.
pub fn harvest(directory: &Path) -> io::Result<(Vec<Reported>, Vec<String>)> {
    let events = directory.join("events");
    let mut names: Vec<String> = match std::fs::read_dir(&events) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json"))
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(error) => return Err(error),
    };
    // Names start with the time and a count, padded: their order is the
    // order the mod wrote them in.
    names.sort();
    let mut reported = Vec::new();
    let mut problems = Vec::new();
    let newest = names.len().saturating_sub(1);
    for (index, name) in names.iter().enumerate() {
        let path = events.join(name);
        let done = path.with_extension("done");
        if !done.exists() && index == newest {
            break;
        }
        match std::fs::read(&path).map(|bytes| serde_json::from_slice::<ModEvent>(&bytes)) {
            Ok(Ok(event)) => {
                remove(&done)?;
                remove(&path)?;
                reported.extend(translate(event));
            }
            Ok(Err(error)) => {
                let damaged = directory.join("damaged");
                ensure_private_dir(&damaged)?;
                std::fs::rename(&path, damaged.join(name))?;
                remove(&done)?;
                problems.push(format!("{} could not be read and was set aside: {error}", path.display()));
            }
            Err(error) => return Err(error),
        }
    }
    Ok((reported, problems))
}

fn translate(event: ModEvent) -> Vec<Reported> {
    let happened = |at: Option<Timestamp>, happened| {
        Reported::Happened(SessionEvent { at: at.unwrap_or_else(Timestamp::now), happened })
    };
    let turn_of = |turn: Option<String>| turn.unwrap_or_default();
    match event {
        ModEvent::TurnStart { turn, at } => vec![happened(at, Happened::TurnStarted { turn: turn_of(turn) })],
        ModEvent::TurnComplete { turn, at, aborted, ms, answer } => {
            let turn = turn_of(turn);
            let mut reported = Vec::new();
            if let Some(text) = &answer {
                let item = format!("{turn}-answer");
                reported.push(happened(at, Happened::Said { turn: turn.clone(), item, text: text.clone() }));
            }
            let how = if aborted { TurnHow::Interrupted } else { TurnHow::Completed };
            let end = TurnEnd { how, reason: None, duration_ms: ms, last_message: answer };
            reported.push(happened(at, Happened::TurnEnded { turn, end }));
            reported
        }
        ModEvent::ToolStart { turn, at, tool, detail } => vec![happened(
            at,
            Happened::ToolStarted {
                turn: turn_of(turn),
                item: String::new(),
                tool: tool.unwrap_or_default(),
                detail: detail.unwrap_or_default(),
            },
        )],
        ModEvent::ToolEnd { turn, at, tool, detail, failed } => vec![happened(
            at,
            Happened::ToolEnded {
                turn: turn_of(turn),
                item: String::new(),
                tool: tool.unwrap_or_default(),
                detail: detail.unwrap_or_default(),
                failed,
            },
        )],
        ModEvent::Trouble { at, file, text } => {
            let text = format!(
                "the World's plugin could not hand over {}: {}",
                file.unwrap_or_default(),
                text.unwrap_or_default()
            );
            vec![happened(at, Happened::Health { turn: None, health: Health { kind: HealthKind::Error, text } })]
        }
        ModEvent::SessionEnd { reason } => vec![Reported::Ended { reason }],
        ModEvent::StopFailed { at, turn, text } => {
            let text = format!("the World's plugin could not stop the turn: {}", text.unwrap_or_default());
            vec![happened(at, Happened::Health { turn, health: Health { kind: HealthKind::Error, text } })]
        }
        ModEvent::Handed { deliveries, turn } => vec![Reported::Handed { ids: deliveries, turn: turn_of(turn) }],
    }
}

/// Something for the session, as the mod reads it.
#[derive(Serialize)]
struct Inbox<'a> {
    id: &'a str,
    text: &'a str,
    /// Words the user typed, which go in as the user's.
    as_user: bool,
}

/// Puts something in the session's inbox for its mod to hand over. The same
/// id twice is the same thing and is written once.
pub fn deliver(directory: &Path, id: &str, text: &str, as_user: bool) -> io::Result<()> {
    let inbox = directory.join("inbox");
    ensure_private_dir(&inbox)?;
    let bytes = serde_json::to_vec(&Inbox { id, text, as_user }).expect("strings serialize");
    match publish_new(&inbox.join(format!("{id}.json")), &bytes, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// Asks the mod to stop `turn`, if it is still the one running when the mod
/// looks.
pub fn stop(directory: &Path, turn: &str) -> io::Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({ "turn": turn })).expect("strings serialize");
    publish(&directory.join("stop.json"), &bytes, Permissions::Private)
}

/// The ids the mod has handed over since the last call; their inbox files
/// and receipts are gone once this returns.
pub fn handed_over(directory: &Path) -> io::Result<Vec<String>> {
    let receipts = directory.join("receipts");
    let mut ids = Vec::new();
    let entries = match std::fs::read_dir(&receipts) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ids),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        remove(&directory.join("inbox").join(format!("{name}.json")))?;
        remove(&receipts.join(&name))?;
        ids.push(name);
    }
    Ok(ids)
}

/// Whether anything handed to the session waits to be handed over.
pub fn waiting(directory: &Path) -> io::Result<bool> {
    match std::fs::read_dir(directory.join("inbox")) {
        Ok(mut entries) => Ok(entries.any(|entry| entry.is_ok_and(|entry| entry.file_name() != ".created"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
