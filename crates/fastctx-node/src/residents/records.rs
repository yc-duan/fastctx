//! A resident's records on this machine (`world-town.md`, 居民目录): who it
//! is (`about`), which project it is in (`town`), what it is doing (`now`),
//! what it did (`actions`), what it saw (`seen`), and for a colleague what it
//! was told and answered (`messages`). Activity uses indexed SQL rows;
//! `about`, `town` and `now` are replaced whole, so a reader sees one
//! version or the next.

use std::io;
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
    super::activity::message(directory, message)
}
pub fn record_action(world: &WorldDir, resident: &ResidentId, action: &Action) -> io::Result<()> {
    super::activity::action(&world.conversation(resident), action)
}
pub fn record_seen(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    seen: &[Seen],
) -> io::Result<()> {
    super::activity::seen(&world.owner(here, owner), seen)
}
pub fn record_owned_action(
    world: &WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    action: &Action,
) -> io::Result<()> {
    super::activity::action(&world.owner(here, owner), action)
}
pub fn last_action(world: &WorldDir, resident: &ResidentId) -> io::Result<Option<Action>> {
    super::activity::last_action(&world.conversation(resident).actions(), None, None)
}
pub fn last_action_before(
    actions: &Path,
    from: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<Action>> {
    super::activity::last_action(actions, from, until)
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
