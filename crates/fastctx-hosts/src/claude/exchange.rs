//! The files the World's mod and the node exchange for one Claude session,
//! in `<world>/claude/<session id>/` (the mod's side is `world.js`).
//!
//! The mod writes `session.json` when the session starts and one event file
//! per turn, tool use, hand-over and end, batching adjacent stream observations.
//! Each report is followed by a `.done` marker once it is whole, since
//! `$.fs.write` promises nothing about being atomic.
//! The node writes what it has for the session into `inbox/`, each file
//! whole before it appears; the mod says in an event which turn took it,
//! and writes a receipt so that it never hands it over again. The node asks
//! for a running turn to be stopped in `stop.json`.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::session::{
    Content, Happened, Health, HealthKind, Item, SessionEvent, ToolStatus, TurnEnd, TurnFacts, TurnHow, What,
};
use fastctx_types::usage::Usd;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod damaged;
pub use damaged::recover_readable;

/// Where the sessions' directories are.
pub fn sessions(world: &Path) -> PathBuf {
    world.join("claude")
}

/// What the mod says of its session when it starts.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Registration {
    pub session: String,
    /// One activation of this session's mod, retained across node restarts.
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub previous: Option<String>,
    #[serde(default)]
    pub at: Option<Timestamp>,
    #[serde(default)]
    pub process: Option<fastctx_types::ProcessIdentity>,
    /// The session's inbox address, for commands from its native shell.
    pub inbox_address: Option<String>,
    /// How the session was started: `cli`, `claude-vscode` and the like.
    pub entrypoint: Option<String>,
    /// The directory it was started in, as Claude Code says.
    #[serde(default)]
    pub cwd: Option<String>,
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

/// Where Claude Code keeps the session's own record, as its hooks told the
/// mod (`transcript.json`); `None` while the mod has not said. The mod's file
/// interface empties the file before it writes it, so an empty one is read
/// as one not yet said.
pub fn transcript(directory: &Path) -> io::Result<Option<PathBuf>> {
    #[derive(Deserialize)]
    struct Place {
        path: PathBuf,
    }
    match std::fs::read(directory.join("transcript.json")) {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => serde_json::from_slice::<Place>(&bytes)
            .map(|place| Some(place.path))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("transcript.json: {error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Says where Claude Code keeps the session's own record, as the mod does
/// when its hooks tell it; for a session whose mod never could.
pub fn name_transcript(directory: &Path, path: &Path) -> io::Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({ "path": path })).map_err(io::Error::other)?;
    publish(&directory.join("transcript.json"), &bytes, Permissions::Private)
}

/// The models a session's mod last observed its login to offer.
#[derive(Deserialize)]
struct Reading {
    at: Timestamp,
    #[serde(default = "available")]
    available: bool,
    models: Vec<fastctx_types::session::control::ModelOffer>,
}

fn available() -> bool {
    true
}

/// The model aliases the mod of the session in `directory` last observed
/// its login to offer, which hold while the session sleeps.
pub fn observed_aliases(directory: &Path) -> io::Result<Option<Vec<String>>> {
    Ok(super::control::read_mod::<Reading>(&directory.join("models.json"))?
        .filter(|reading| reading.available)
        .map(|reading| reading.models.into_iter().map(|model| model.id).collect()))
}

/// The models each session's login offers, from the aliases its mod observed, attributed by transcript home.
pub fn models(world: &Path) -> io::Result<Vec<(PathBuf, Timestamp, Vec<fastctx_types::agents::AgentModel>)>> {
    let mut readings = Vec::new();
    let entries = match std::fs::read_dir(sessions(world)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(readings),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let directory = entry?.path();
        let Some(reading) = super::control::read_mod::<Reading>(&directory.join("models.json"))? else { continue };
        if !reading.available {
            continue;
        }
        let Some(transcript) = transcript(&directory)? else { continue };
        let aliases: Vec<String> = reading.models.into_iter().map(|model| model.id).collect();
        let models = super::models::agent_models(Some(&aliases));
        readings.push((transcript, reading.at, models));
    }
    readings.sort_by(|(_, a, _), (_, b, _)| b.cmp(a));
    Ok(readings)
}

/// What the mod reported, in the World's terms.
#[derive(Debug)]
pub enum Reported {
    Uncertain {
        ids: Vec<String>,
        text: String,
    },
    Happened(SessionEvent, Option<String>),
    /// The conversation ended, including /clear or resume within the process.
    Ended {
        reason: Option<String>,
        instance: Option<String>,
    },
    /// The mod handed these inbox files to the model, in `turn`.
    Handed {
        ids: Vec<String>,
        turn: String,
    },
    /// These inbox files were replaced by what came after them before they
    /// went in: they never will.
    Replaced {
        ids: Vec<String>,
    },
    /// Claude Code kept this record of the main conversation; its file has
    /// it only a moment later (`crate::claude::transcript`).
    Stored {
        uuid: String,
    },
    /// The stored record `uuid` ended the live thinking item `item` of
    /// `turn`; how long it took and what its request cost are on that record
    /// once the file has it (`crate::claude::thoughts`).
    Thought {
        uuid: String,
        turn: String,
        item: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum ModEvent {
    Agent {
        agent_id: String,
        report: Box<ModEvent>,
    },
    Batch {
        events: Vec<ModEvent>,
    },
    /// Claude Code kept the main conversation's record `uuid`.
    Stored {
        uuid: String,
    },
    /// The main conversation's record `uuid` ended the live thinking item
    /// `item` of `turn`.
    Thought {
        uuid: String,
        turn: String,
        item: String,
    },
    Session {
        at: Option<Timestamp>,
        #[serde(deserialize_with = "observed_happened")]
        happened: Happened,
    },
    ToolStored {
        at: Option<Timestamp>,
        turn: Option<String>,
        item: String,
        tool: String,
        input: Value,
        started_at: Timestamp,
        content: Value,
        failed: bool,
        declined: bool,
        #[serde(default)]
        cut: bool,
        #[serde(default, deserialize_with = "elapsed_milliseconds")]
        ms: Option<u64>,
        detail: Value,
    },
    Uncertain {
        deliveries: Vec<String>,
        text: String,
    },
    TurnStart {
        turn: Option<String>,
        at: Option<Timestamp>,
        #[serde(default)]
        activation: Option<String>,
    },
    TurnComplete {
        turn: Option<String>,
        at: Option<Timestamp>,
        aborted: bool,
        reason: Option<String>,
        refusal: Option<Refusal>,
        #[serde(default)]
        error: Option<Value>,
        #[serde(default, deserialize_with = "elapsed_milliseconds")]
        ms: Option<u64>,
        answer: Option<String>,
        #[serde(default)]
        streamed: bool,
        /// What the turn cost by Claude Code's own cost ledger, in dollars:
        /// how much the session's total rose over it.
        #[serde(default)]
        spent: Option<f64>,
    },
    /// A tool use began: its arguments, as the tool receives them.
    ToolStart {
        turn: Option<String>,
        at: Option<Timestamp>,
        item: Option<String>,
        tool: Option<String>,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        started_at: Option<Timestamp>,
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
        #[serde(default, deserialize_with = "elapsed_milliseconds")]
        ms: Option<u64>,
    },
    Trouble {
        at: Option<Timestamp>,
        file: Option<String>,
        text: Option<String>,
    },
    SessionEnd {
        reason: Option<String>,
        #[serde(default)]
        instance: Option<String>,
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
    Replaced {
        deliveries: Vec<String>,
    },
}

// Claude uses its wall clock for some durations. A clock correction can
// invalidate that measurement without invalidating the terminal event.
fn elapsed_milliseconds<'de, D: serde::Deserializer<'de>>(input: D) -> Result<Option<u64>, D::Error> {
    Ok(Option::<serde_json::Number>::deserialize(input)?.and_then(|number| number.as_u64()))
}

fn observed_happened<'de, D: serde::Deserializer<'de>>(input: D) -> Result<Happened, D::Error> {
    let mut value = Value::deserialize(input)?;
    let duration = match value["event"].as_str() {
        Some("turn_ended") => value.pointer_mut("/end/duration_ms"),
        Some("changed") if value["change"]["kind"] == "agent_ended" => value.pointer_mut("/change/end/duration_ms"),
        _ => None,
    };
    if let Some(duration) = duration
        && duration.is_number()
        && duration.as_u64().is_none()
    {
        *duration = Value::Null;
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
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
    super::accounting::clean(directory, name.as_ref())?;
    let path = directory.join("events").join(name);
    remove(&path)?;
    remove(&path.with_extension("done"))?;
    match std::fs::remove_dir_all(path.with_extension("parts")) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Takes every whole event the mod wrote, oldest first. An event that
/// cannot be read is moved to `damaged/` and said in the problems; the
/// first without its marker blocks later reports until its write completes.
pub fn harvest(directory: &Path) -> io::Result<(Vec<Report>, Vec<String>)> {
    let mut problems = super::accounting::promote(directory, false)?;
    let events = directory.join("events");
    let mut names: Vec<String> = match std::fs::read_dir(&events) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json"))
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), problems)),
        Err(error) => return Err(error),
    };
    // Names start with the time and a count, padded: their order is the
    // order the mod wrote them in.
    names.sort();
    let mut reported = Vec::new();
    for name in &names {
        if directory.join("applied").join(name).exists() {
            acknowledge(directory, name)?;
            continue;
        }
        let path = events.join(name);
        let done = path.with_extension("done");
        if !done.exists() {
            break;
        }
        match read_report(&path).map(|bytes| serde_json::from_slice::<ModEvent>(&bytes)) {
            Ok(Ok(event)) => {
                let at = Timestamp::try_from(path.metadata()?.modified()?).map_err(io::Error::other)?;
                reported.push(Report { name: name.clone(), events: translate(event, at) });
            }
            Ok(Err(error)) => {
                let damaged = directory.join("damaged");
                ensure_private_dir(&damaged)?;
                fastctx_platform::owned_directory::move_to(&path, &damaged.join(name))?;
                let parts = path.with_extension("parts");
                if parts.try_exists()? {
                    fastctx_platform::owned_directory::move_to(
                        &parts,
                        &damaged.join(parts.file_name().expect("event parts")),
                    )?;
                }
                remove(&done)?;
                problems.push(format!("{} could not be read and was set aside: {error}", path.display()));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => {
                problems.push(format!("{} could not be read: {error}", path.display()));
                break;
            }
        }
    }
    Ok((reported, problems))
}

/// The writer is known dead: a complete JSON report no longer needs its
/// final marker to distinguish it from an in-progress write.
pub fn recover_reports(directory: &Path) -> io::Result<Vec<String>> {
    let mut problems = super::accounting::promote(directory, true)?;
    let entries = match std::fs::read_dir(directory.join("events")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(problems),
        Err(error) => return Err(error),
    };
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

pub(super) fn checked_report(path: &Path) -> io::Result<Vec<u8>> {
    let bytes = read_report(path)?;
    serde_json::from_slice::<ModEvent>(&bytes).map_err(io::Error::other)?;
    Ok(bytes)
}

fn read_report(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = std::fs::read(path)?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if let Some(parts) = value.get("parts").and_then(Value::as_u64) {
        bytes.clear();
        for index in 0..parts {
            bytes.extend(std::fs::read(path.with_extension("parts").join(index.to_string()))?);
        }
        value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    }
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
    Ok(bytes)
}

fn translate(event: ModEvent, recorded_at: Timestamp) -> Vec<Reported> {
    let happened = |at: Option<Timestamp>, happened| {
        Reported::Happened(SessionEvent { at: at.unwrap_or(recorded_at), happened }, None)
    };
    let turn_of = |turn: Option<String>| turn.unwrap_or_default();
    match event {
        ModEvent::Agent { agent_id, report } => translate(*report, recorded_at)
            .into_iter()
            .filter_map(|report| {
                if let Reported::Happened(event, _) = report {
                    Some(Reported::Happened(
                        SessionEvent {
                            at: event.at,
                            happened: Happened::Changed {
                                turn: None,
                                change: fastctx_types::session::SessionChange::AgentEvent {
                                    agent_id: agent_id.clone(),
                                    happened: Box::new(event.happened),
                                },
                            },
                        },
                        None,
                    ))
                } else {
                    None
                }
            })
            .collect(),
        ModEvent::Batch { events } => events.into_iter().flat_map(|event| translate(event, recorded_at)).collect(),
        ModEvent::Stored { uuid } => vec![Reported::Stored { uuid }],
        ModEvent::Thought { uuid, turn, item } => vec![Reported::Thought { uuid, turn, item }],
        ModEvent::Session { at, happened: observed } => vec![happened(at, observed)],
        ModEvent::ToolStored {
            at,
            turn,
            item,
            tool,
            input,
            started_at,
            content,
            failed,
            declined,
            cut,
            ms,
            detail,
        } => {
            let status = if declined {
                ToolStatus::Declined
            } else if failed {
                ToolStatus::Failed
            } else if cut {
                ToolStatus::Cut
            } else {
                ToolStatus::Done
            };
            let output = super::content::content(&content);
            let tool = super::tools::tool(&tool, input, output, status, ms);
            let item = Item {
                draft: None,
                id: item,
                started_at: Some(started_at),
                detail: Some(detail),
                what: What::Tool(Box::new(tool)),
            };
            vec![happened(at, Happened::Finished { turn: turn_of(turn), item })]
        }
        ModEvent::Uncertain { deliveries, text } => vec![Reported::Uncertain { ids: deliveries, text }],
        ModEvent::TurnStart { turn, at, activation } => vec![Reported::Happened(
            SessionEvent { at: at.unwrap_or(recorded_at), happened: Happened::TurnStarted { turn: turn_of(turn) } },
            activation,
        )],
        ModEvent::TurnComplete { turn, at, aborted, reason, refusal, error, ms, answer, streamed, spent } => {
            let turn = turn_of(turn);
            let mut reported = Vec::new();
            // Legacy mod reports had no completed blocks. Never manufacture
            // a second message from the host's trimmed summary of a stream.
            if !streamed && let Some(text) = &answer {
                let item = Item {
                    draft: None,
                    id: format!("{turn}-answer"),
                    started_at: None,
                    detail: None,
                    what: What::Message { text: text.clone() },
                };
                reported.push(happened(at, Happened::Finished { turn: turn.clone(), item }));
            }
            let error = error.or_else(|| refusal.as_ref().map(|refusal| serde_json::json!({
                "category": refusal.category.as_deref().unwrap_or("refusal"), "message": refusal.explanation,
            })));
            // Cancellation determines how the turn ended, even when it also carries a diagnostic.
            let how = match reason.as_deref() {
                _ if aborted => TurnHow::Interrupted,
                Some("aborted") => TurnHow::Interrupted,
                _ if error.is_some() => TurnHow::Failed,
                Some("answer") | None => TurnHow::Completed,
                Some("refusal" | "error") => TurnHow::Failed,
                Some(other) => TurnHow::Other(other.to_string()),
            };
            let reason = match refusal {
                Some(Refusal { category: Some(category), explanation: Some(explanation) }) => {
                    Some(format!("{category}: {explanation}"))
                }
                Some(Refusal { category, explanation }) => explanation.or(category).or(reason),
                // How the turn ended says these already.
                None if matches!(reason.as_deref(), Some("answer" | "aborted")) => None,
                None => reason,
            };
            // The rest of what the turn took the node has from each request.
            let facts = spent
                .and_then(Usd::from_dollars)
                .map(|cost| Box::new(TurnFacts { cost: Some(cost), ..TurnFacts::default() }));
            let reason = error.as_ref().and_then(|error| error["message"].as_str()).map(str::to_owned).or(reason);
            let end = TurnEnd { facts, error, how, reason, duration_ms: ms, last_message: answer };
            reported.push(happened(at, Happened::TurnEnded { turn, end }));
            reported
        }
        ModEvent::ToolStart { turn, at, item, tool, input, started_at } => {
            let tool = super::tools::tool(&tool.unwrap_or_default(), input, Vec::new(), ToolStatus::Running, None);
            let item = Item {
                draft: None,
                id: item.unwrap_or_default(),
                started_at,
                detail: None,
                what: What::Tool(Box::new(tool)),
            };
            vec![happened(at, Happened::Began { turn: turn_of(turn), item })]
        }
        ModEvent::ToolEnd { turn, at, item, tool, input, output, failed, declined, ms } => {
            let status = match (declined, failed) {
                (true, _) => ToolStatus::Declined,
                (false, true) => ToolStatus::Failed,
                (false, false) => ToolStatus::Done,
            };
            let tool = super::tools::tool(&tool.unwrap_or_default(), input, output, status, ms);
            let item = Item {
                draft: None,
                id: item.unwrap_or_default(),
                started_at: None,
                detail: None,
                what: What::Tool(Box::new(tool)),
            };
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
        ModEvent::SessionEnd { reason, instance } => vec![Reported::Ended { reason, instance }],
        ModEvent::StopFailed { at, turn, text } => {
            let text = format!("the World's plugin could not stop the turn: {}", text.unwrap_or_default());
            vec![happened(at, Happened::Health { turn, health: Health { kind: HealthKind::Error, text } })]
        }
        ModEvent::Handed { deliveries, turn } => vec![Reported::Handed { ids: deliveries, turn: turn_of(turn) }],
        ModEvent::Replaced { deliveries } => vec![Reported::Replaced { ids: deliveries }],
    }
}

/// Something for the session, as the mod reads it.
#[derive(Serialize)]
struct Inbox<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<Timestamp>,
    text: &'a str,
    /// Words the user typed, which go in as the user's.
    as_user: bool,
    /// It goes in with the next turn and starts none; its kind, of which
    /// only the latest goes in (`Notice::along`).
    #[serde(skip_serializing_if = "Option::is_none")]
    along: Option<&'a str>,
    /// What it replaces, handed over before and not yet in.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    replaces: &'a [String],
}

/// How one thing goes into the session: as the user's words, or with the
/// next turn in place of what it replaces.
#[derive(Default)]
pub struct Delivering<'a> {
    pub at: Option<Timestamp>,
    pub as_user: bool,
    pub along: Option<&'a str>,
    pub replaces: &'a [String],
}

/// Puts something in the session's inbox for its mod to hand over. The same
/// id twice is the same thing and is written once.
pub fn deliver(directory: &Path, id: &str, text: &str, how: Delivering<'_>) -> io::Result<()> {
    let Delivering { at, as_user, along, replaces } = how;
    let inbox = directory.join("inbox");
    ensure_private_dir(&inbox)?;
    let mut bytes = serde_json::to_vec(&Inbox { id, at, text, as_user, along, replaces }).expect("strings serialize");
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
        let mut value = serde_json::json!({ "id": id, "text": "", "text_parts": count, "as_user": as_user });
        if let Some(at) = at {
            value["at"] = serde_json::to_value(at).map_err(io::Error::other)?;
        }
        if let Some(along) = along {
            value["along"] = along.into();
        }
        if !replaces.is_empty() {
            value["replaces"] = replaces.into();
        }
        bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
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

/// The ids the mod has handed over; their inbox files and assembled parts
/// are gone once this returns. Receipts remain as duplicate-delivery proof.
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
        let parts = directory.join("inbox-parts").join(&name);
        match std::fs::read_dir(&parts) {
            Ok(entries) => {
                for part in entries {
                    let part = part?;
                    if part.file_name().to_str().is_some_and(|name| name.parse::<usize>().is_ok()) {
                        remove(&part.path())?;
                    }
                }
                std::fs::remove_dir(&parts)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
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
