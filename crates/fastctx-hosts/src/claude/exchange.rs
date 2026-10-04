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
use fastctx_types::session::{
    Content, Happened, Health, HealthKind, Item, SessionEvent, ToolStatus, TurnEnd, TurnHow, What,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the sessions' directories are.
pub fn sessions(world: &Path) -> PathBuf {
    world.join("claude")
}

/// What the mod says of its session when it starts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Registration {
    pub session: String,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub at: Option<Timestamp>,
    #[serde(default)]
    pub process: Option<fastctx_types::ProcessIdentity>,
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
    Uncertain {
        ids: Vec<String>,
        text: String,
    },
    Happened(SessionEvent),
    /// The conversation ended, including /clear or resume within the process.
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
    Uncertain {
        deliveries: Vec<String>,
        text: String,
    },
    TurnStart {
        turn: Option<String>,
        at: Option<Timestamp>,
    },
    TurnComplete {
        turn: Option<String>,
        at: Option<Timestamp>,
        aborted: bool,
        reason: Option<String>,
        refusal: Option<Refusal>,
        ms: Option<u64>,
        answer: Option<String>,
    },
    /// A tool use began: its arguments, as the tool receives them.
    ToolStart {
        turn: Option<String>,
        at: Option<Timestamp>,
        item: Option<String>,
        tool: Option<String>,
        #[serde(default)]
        input: Value,
    },
    /// A tool use ended: what it gave back, as the model reads it.
    ToolEnd {
        turn: Option<String>,
        at: Option<Timestamp>,
        item: Option<String>,
        tool: Option<String>,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        output: Vec<Content>,
        failed: bool,
        #[serde(default)]
        declined: bool,
        #[serde(default)]
        ms: Option<u64>,
    },
    Trouble {
        at: Option<Timestamp>,
        file: Option<String>,
        text: Option<String>,
    },
    SessionEnd {
        reason: Option<String>,
    },
    StopFailed {
        at: Option<Timestamp>,
        turn: Option<String>,
        text: Option<String>,
    },
    Handed {
        deliveries: Vec<String>,
        turn: Option<String>,
    },
}

#[derive(Deserialize)]
struct Refusal {
    category: Option<String>,
    explanation: Option<String>,
}

/// A source report remains present until every translated event is durable.
pub struct Report {
    pub name: String,
    pub events: Vec<Reported>,
}

pub fn acknowledge(directory: &Path, name: &str) -> io::Result<()> {
    let applied = directory.join("applied");
    ensure_private_dir(&applied)?;
    match publish_new(&applied.join(name), b"", Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    let path = directory.join("events").join(name);
    remove(&path)?;
    remove(&path.with_extension("done"))
}

/// Takes every whole event the mod wrote, oldest first. An event that
/// cannot be read is moved to `damaged/` and said in the problems; the
/// newest without its marker is left, as it may still be being written.
pub fn harvest(directory: &Path) -> io::Result<(Vec<Report>, Vec<String>)> {
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
    for name in &names {
        let path = events.join(name);
        let done = path.with_extension("done");
        if !done.exists() {
            continue;
        }
        match read_report(&path).map(|bytes| serde_json::from_slice::<ModEvent>(&bytes)) {
            Ok(Ok(event)) => {
                let at = Timestamp::try_from(path.metadata()?.modified()?).map_err(io::Error::other)?;
                reported.push(Report { name: name.clone(), events: translate(event, at) });
            }
            Ok(Err(error)) => {
                let damaged = directory.join("damaged");
                ensure_private_dir(&damaged)?;
                std::fs::rename(&path, damaged.join(name))?;
                remove(&done)?;
                problems.push(format!("{} could not be read and was set aside: {error}", path.display()));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => problems.push(format!("{} could not be read: {error}", path.display())),
        }
    }
    Ok((reported, problems))
}

/// The writer is known dead: a complete JSON report no longer needs its
/// final marker to distinguish it from an in-progress write.
pub fn recover_reports(directory: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(directory.join("events")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut problems = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") || path.with_extension("done").exists() {
            continue;
        }
        match read_report(&path).and_then(|bytes| serde_json::from_slice::<ModEvent>(&bytes).map_err(io::Error::other))
        {
            Ok(_) => publish(&path.with_extension("done"), b"", Permissions::Private)?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => problems.push(format!("{} was incomplete when its host exited: {error}", path.display())),
        }
    }
    Ok(problems)
}

fn read_report(path: &Path) -> io::Result<Vec<u8>> {
    let bytes = std::fs::read(path)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if let Some(pending) = value.get("pending").and_then(Value::as_array) {
        let applied = path.parent().and_then(Path::parent).expect("an event is inside a session").join("applied");
        for name in pending {
            let name = name.as_str().ok_or_else(|| io::Error::other("invalid report dependency"))?;
            if Path::new(name).file_name().is_none_or(|file| file != name) {
                return Err(io::Error::other("invalid report dependency path"));
            }
            if !applied.join(name).exists() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
        }
    }
    let Some(parts) = value.get("parts").and_then(Value::as_u64) else { return Ok(bytes) };
    let mut body = Vec::new();
    for index in 0..parts {
        body.extend(std::fs::read(path.with_extension("parts").join(index.to_string()))?);
    }
    Ok(body)
}

fn translate(event: ModEvent, recorded_at: Timestamp) -> Vec<Reported> {
    let happened =
        |at: Option<Timestamp>, happened| Reported::Happened(SessionEvent { at: at.unwrap_or(recorded_at), happened });
    let turn_of = |turn: Option<String>| turn.unwrap_or_default();
    match event {
        ModEvent::Uncertain { deliveries, text } => vec![Reported::Uncertain { ids: deliveries, text }],
        ModEvent::TurnStart { turn, at } => vec![happened(at, Happened::TurnStarted { turn: turn_of(turn) })],
        ModEvent::TurnComplete { turn, at, aborted, reason, refusal, ms, answer } => {
            let turn = turn_of(turn);
            let mut reported = Vec::new();
            if let Some(text) = &answer {
                let item = Item { id: format!("{turn}-answer"), what: What::Message { text: text.clone() } };
                reported.push(happened(at, Happened::Finished { turn: turn.clone(), item }));
            }
            let how = match reason.as_deref() {
                Some("answer") => TurnHow::Completed,
                Some("aborted") => TurnHow::Interrupted,
                Some("refusal" | "error") => TurnHow::Failed,
                Some(other) => TurnHow::Other(other.to_string()),
                // Events already on disk from the previous mod have only this flag.
                None => {
                    if aborted {
                        TurnHow::Interrupted
                    } else {
                        TurnHow::Completed
                    }
                }
            };
            let reason = match refusal {
                Some(Refusal { category: Some(category), explanation: Some(explanation) }) => {
                    Some(format!("{category}: {explanation}"))
                }
                Some(Refusal { category, explanation }) => explanation.or(category).or(reason),
                None if reason.as_deref() == Some("answer") => None,
                None => reason,
            };
            let end = TurnEnd { how, reason, duration_ms: ms, last_message: answer };
            reported.push(happened(at, Happened::TurnEnded { turn, end }));
            reported
        }
        ModEvent::ToolStart { turn, at, item, tool, input } => {
            let tool = super::tools::tool(&tool.unwrap_or_default(), input, Vec::new(), ToolStatus::Running, None);
            let item = Item { id: item.unwrap_or_default(), what: What::Tool(Box::new(tool)) };
            vec![happened(at, Happened::Began { turn: turn_of(turn), item })]
        }
        ModEvent::ToolEnd { turn, at, item, tool, input, output, failed, declined, ms } => {
            let status = match (declined, failed) {
                (true, _) => ToolStatus::Declined,
                (false, true) => ToolStatus::Failed,
                (false, false) => ToolStatus::Done,
            };
            let tool = super::tools::tool(&tool.unwrap_or_default(), input, output, status, ms);
            let item = Item { id: item.unwrap_or_default(), what: What::Tool(Box::new(tool)) };
            vec![happened(at, Happened::Finished { turn: turn_of(turn), item })]
        }
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
    let mut bytes = serde_json::to_vec(&Inbox { id, text, as_user }).expect("strings serialize");
    // Claude Code 2.1.287's fs API accepts at most 4 MiB per operation.
    const FILE_BYTES: usize = 4 * 1024 * 1024;
    if bytes.len() > FILE_BYTES {
        let parts = directory.join("inbox-parts").join(id);
        ensure_private_dir(&parts)?;
        let mut rest = text;
        let mut count = 0usize;
        while !rest.is_empty() {
            let mut end = rest.len().min(FILE_BYTES);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let part = &rest[..end];
            let path = parts.join(count.to_string());
            match publish_new(&path, part.as_bytes(), Permissions::Private) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&path)? != part.as_bytes() {
                        return Err(io::Error::other("a delivery changed under the same id"));
                    }
                }
                Err(error) => return Err(error),
            }
            rest = &rest[end..];
            count += 1;
        }
        bytes =
            serde_json::to_vec(&serde_json::json!({ "id": id, "text": "", "text_parts": count, "as_user": as_user }))
                .map_err(io::Error::other)?;
    }
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

pub fn acknowledge_handover(directory: &Path, ids: &[String], turn: &str) -> io::Result<()> {
    let receipts = directory.join("receipts");
    ensure_private_dir(&receipts)?;
    for id in ids {
        let id = fastctx_types::NoticeId::try_from(id.as_str()).map_err(io::Error::other)?;
        let bytes = serde_json::to_vec(&serde_json::json!({ "turn": turn })).map_err(io::Error::other)?;
        match publish_new(&receipts.join(id.as_str()), &bytes, Permissions::Private) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// Receipts are a second durable copy of the consuming-turn proof.
pub fn receipts(directory: &Path) -> io::Result<Vec<(String, String)>> {
    let mut accepted = Vec::new();
    let entries = match std::fs::read_dir(directory.join("receipts")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(accepted),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let value: Value = serde_json::from_slice(&std::fs::read(entry.path())?).map_err(io::Error::other)?;
        let turn = value.get("turn").and_then(Value::as_str).filter(|turn| !turn.is_empty()).map(str::to_string);
        if let Some(turn) = turn {
            accepted.push((name, turn));
        }
    }
    Ok(accepted)
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
        // Keep the receipt across mod reload and duplicate delivery.
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
