//! A resident's records on this machine (`world-town.md`, 居民目录): who it
//! is (`about`), which project it is in (`town`), what it is doing (`now`),
//! what it did (`actions`), what it saw (`seen`), and for a colleague what it
//! was told and answered (`messages`). Each line file grows by whole lines;
//! `about`, `town` and `now` are replaced whole, so a reader sees one
//! version or the next.

use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::layout::{ConversationDir, WorldDir};
use fastctx_types::residents::{About, Action, MessageLine, Now, Places, Seen};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

/// Writes a new resident's `about` and the project it starts in; fails if
/// it has an `about`.
pub fn create(directory: &ConversationDir, about: &About, places: &Places) -> io::Result<()> {
    ensure_private_dir(directory.path())?;
    // Its project first: a directory with an `about` is a resident, and a
    // resident's project is always known.
    write_places(directory, places)?;
    publish_new(&directory.about(), &json(about)?, Permissions::Private)
}

/// Replaces a resident's `about`, as when its host first tells the World
/// which session it is.
pub fn write_about(directory: &ConversationDir, about: &About) -> io::Result<()> {
    publish(&directory.about(), &json(about)?, Permissions::Private)
}

pub fn read_about(directory: &ConversationDir) -> io::Result<About> {
    read_json(&directory.about())
}

/// The projects the resident has been in. One written before projects were
/// recorded apart was born in no project.
pub fn read_places(directory: &ConversationDir, about: &About) -> io::Result<Places> {
    match read_json(&directory.town()) {
        Ok(places) => Ok(places),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Places::born(None, about.born_at)),
        Err(error) => Err(error),
    }
}

/// Replaces the resident's projects whole: changing projects is this one
/// write, which either happened or did not.
pub fn write_places(directory: &ConversationDir, places: &Places) -> io::Result<()> {
    publish(&directory.town(), &json(places)?, Permissions::Private)
}

/// When the user removed the resident from the World, if they did.
pub fn read_removed(directory: &ConversationDir) -> io::Result<Option<Timestamp>> {
    match read_json(&directory.removed()) {
        Ok(at) => Ok(Some(at)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The resident's `now`; `None` when its host never told the World.
pub fn read_now(directory: &ConversationDir) -> io::Result<Option<Now>> {
    match read_json::<serde_json::Value>(&directory.now()) {
        Ok(mut value) => {
            // Earlier nodes stored a single turn's readers as an array.
            if value.get("owed").is_some_and(serde_json::Value::is_array) {
                let readers = value["owed"].take();
                let turn = value.get("turn").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                value["owed"] = if readers.as_array().is_some_and(Vec::is_empty) {
                    serde_json::json!({})
                } else {
                    serde_json::json!({ turn: readers })
                };
            }
            serde_json::from_value(value).map(Some).map_err(io::Error::other)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_now(directory: &ConversationDir, now: &Now) -> io::Result<()> {
    publish(&directory.now(), &json(now)?, Permissions::Private)
}

pub fn append_message(directory: &ConversationDir, message: &MessageLine) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    append_lines(&directory.messages(), std::slice::from_ref(message))
}

/// Appends one action to a resident's `actions`.
pub fn record_action(world: &WorldDir, resident: &ResidentId, action: &Action) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    append_lines(&world.conversation(resident).actions(), std::slice::from_ref(action))
}

/// Appends what a resident saw on this machine to its `seen` record here,
/// one line per file, all in one write so each call's lines stay together.
pub fn record_seen(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    seen: &[Seen],
) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let directory = world.owner(here, owner);
    ensure_private_dir(&directory.books())?;
    append_lines(&directory.seen(), seen)
}

/// Records a shell write where it happened, including a visiting resident.
pub fn record_owned_action(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    action: &Action,
) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let directory = world.owner(here, owner);
    ensure_private_dir(directory.path())?;
    append_lines(&directory.actions(), std::slice::from_ref(action))
}

/// A resident's latest action: the last line of its `actions`, read from
/// the end, so a record of any length costs the same.
pub fn last_action(world: &WorldDir, resident: &ResidentId) -> io::Result<Option<Action>> {
    last_action_before(&world.conversation(resident).actions(), None, None)
}

/// The latest action in `actions` from `from` on and before `until`, read
/// from the end until one is found or the lines are older than `from`.
pub fn last_action_before(
    actions: &Path,
    from: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<Action>> {
    let mut file = match std::fs::File::open(actions) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut end = file.metadata()?.len();
    // Lines are read back from the end; each step reads what lies before
    // the lines already looked at.
    let mut carried = Vec::new();
    loop {
        let step = end.min(4096);
        let start = end - step;
        file.seek(SeekFrom::Start(start))?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&carried);
        let mut body: &[u8] = &chunk;
        // Whole lines in what is held: those after the first newline, or
        // everything once the start of the file is reached.
        loop {
            let trimmed = body.strip_suffix(b"\n").unwrap_or(body);
            let (rest, line) = match trimmed.iter().rposition(|&b| b == b'\n') {
                Some(newline) => (&trimmed[..=newline], &trimmed[newline + 1..]),
                None if start == 0 => (&trimmed[..0], trimmed),
                None => break,
            };
            if !line.is_empty() {
                let action: Action = parse(actions, line)?;
                if from.is_some_and(|from| action.at < from) {
                    return Ok(None);
                }
                if until.is_none_or(|until| action.at < until) {
                    return Ok(Some(action));
                }
            }
            body = rest;
            if body.is_empty() {
                break;
            }
        }
        if start == 0 {
            return Ok(None);
        }
        carried = body.to_vec();
        end = start;
    }
}

fn append_lines<T: serde::Serialize>(path: &Path, lines: &[T]) -> io::Result<()> {
    let mut bytes = Vec::new();
    for line in lines {
        serde_json::to_writer(&mut bytes, line).map_err(io::Error::other)?;
        bytes.push(b'\n');
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&bytes)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    parse(path, &std::fs::read(path)?)
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path, bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn json(value: &impl serde::Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}
