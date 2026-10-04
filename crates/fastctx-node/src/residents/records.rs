//! A resident's records on this machine (`world-town.md`, 居民目录): who it
//! is (`about`), what it is doing (`now`), what it did (`actions`), what it
//! saw (`seen`), and for a colleague what it was told and answered
//! (`messages`). Each line file grows by whole lines; `about` is written
//! once; `now` is replaced whole, so a reader sees one version or the next.

use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::layout::{ResidentDir, WorldDir};
use fastctx_types::residents::{About, Action, MessageLine, Now, Seen};
use fastctx_types::{Address, MachineName, ResidentId, TownName};

/// Writes a new resident's `about`; fails if it has one.
pub fn create(directory: &ResidentDir, about: &About) -> io::Result<()> {
    ensure_private_dir(directory.path())?;
    publish_new(&directory.about(), &json(about)?, Permissions::Private)
}

pub fn read_about(directory: &ResidentDir) -> io::Result<About> {
    read_json(&directory.about())
}

/// The resident's `now`; `None` when its host never told the World.
pub fn read_now(directory: &ResidentDir) -> io::Result<Option<Now>> {
    match read_json(&directory.now()) {
        Ok(now) => Ok(Some(now)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_now(directory: &ResidentDir, now: &Now) -> io::Result<()> {
    publish(&directory.now(), &json(now)?, Permissions::Private)
}

pub fn append_message(directory: &ResidentDir, message: &MessageLine) -> io::Result<()> {
    append_lines(&directory.messages(), std::slice::from_ref(message))
}

/// Appends one action to a resident's `actions`.
pub fn record_action(
    world: &WorldDir,
    resident: &ResidentId,
    town: Option<&TownName>,
    action: &Action,
) -> io::Result<()> {
    append_lines(&world.resident(town, resident).actions(), std::slice::from_ref(action))
}

/// Appends what a resident saw on this machine to its `seen` record here,
/// one line per file, all in one write so each call's lines stay together.
pub fn record_seen(
    world: &WorldDir,
    here: &MachineName,
    owner: &Address,
    town: Option<&TownName>,
    seen: &[Seen],
) -> io::Result<()> {
    let directory = world.owner(here, town, owner);
    ensure_private_dir(directory.path())?;
    append_lines(&directory.seen(), seen)
}

/// A resident's latest action: the last line of its `actions`, read from
/// the end, so a record of any length costs the same.
pub fn last_action(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>) -> io::Result<Option<Action>> {
    let path = world.resident(town, resident).actions();
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    // Read back from the end until a whole last line is in hand.
    let mut tail = Vec::new();
    let mut from = length;
    loop {
        let step = from.min(4096);
        from -= step;
        file.seek(SeekFrom::Start(from))?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        let body = tail.strip_suffix(b"\n").unwrap_or(&tail);
        if let Some(newline) = body.iter().rposition(|&b| b == b'\n') {
            return parse(&path, &body[newline + 1..]).map(Some);
        }
        if from == 0 {
            return if body.is_empty() { Ok(None) } else { parse(&path, body).map(Some) };
        }
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
