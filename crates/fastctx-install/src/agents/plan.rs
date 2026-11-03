//! Working out what connecting an agent, or taking it out, changes, with
//! nothing read or written: the files as they are in, each file's bytes
//! after and the receipt out, with the steps the window before the change
//! tells (`world-layers.md`, 接入之前讲清楚做了什么). Its server's entry
//! comes first, then its shell, its limits, and the passage in its
//! instructions, each told as its own kind of change.
//!
//! Where an agent reads only one of several files (`Pick`), each change
//! goes to the one it reads now; one Fastctx made earlier in another is
//! taken out of there first, so the agent always finds it.

use std::collections::{BTreeMap, BTreeSet};

use fastctx_types::logins::Change;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::json::{self, Added, OwnFile, Placed};
use super::spec::{Agent, Edit, Format, Pick, Place, Server, Slot, To};
use super::yaml;
use crate::connect::Steps;
use crate::{Error, File, Notice, guidance};

/// What connecting an agent wrote, to take out again what is still as it
/// was written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server: Option<Held>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shell: Vec<Option<Held>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    limits: Vec<Option<Held>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guidance: Option<guidance::Receipt>,
}

impl Receipt {
    /// Whether anything of Fastctx's is held: the agent is connected.
    pub fn holds(&self) -> bool {
        self.server.is_some()
            || self.shell.iter().any(Option::is_some)
            || self.limits.iter().any(Option::is_some)
            || self.guidance.is_some()
    }

    /// The file Fastctx's server is registered in, relative to the home.
    pub fn server_file(&self) -> Option<&str> {
        self.server.as_ref().map(|held| held.file.as_str())
    }
}

/// One place of Fastctx's: the file it is in, relative to the home, and
/// what it is there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Held {
    file: String,
    owned: Owned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Owned {
    Set(Placed),
    Add(Added),
    Own(OwnFile),
    YamlSet(yaml::Placed),
    YamlAdd(yaml::Added),
}

/// The files connecting reads and writes, by their place in the agent's
/// home: each one's path as receipts and the log name it, and its bytes,
/// `None` where there is no file.
pub type Files = BTreeMap<&'static str, (String, Option<Vec<u8>>)>;

/// A change worked out: each file's bytes after (`None`: no file), the
/// receipt to keep, what was left alone and why, and the steps told.
pub struct Planned {
    pub files: BTreeMap<&'static str, Option<Vec<u8>>>,
    pub receipt: Receipt,
    pub notices: Vec<Notice>,
    pub steps: Steps,
}

/// The files as the steps leave them, each step taking one file on.
struct Working<'a> {
    files: &'a Files,
    now: BTreeMap<&'static str, Option<Vec<u8>>>,
    steps: Steps,
}

impl<'a> Working<'a> {
    fn new(files: &'a Files) -> Self {
        Self {
            files,
            now: files.iter().map(|(name, (_, bytes))| (*name, bytes.clone())).collect(),
            steps: Steps::default(),
        }
    }

    fn path(&self, file: &str) -> Result<&'a str, Error> {
        self.files.get(file).map(|(path, _)| path.as_str()).ok_or_else(|| Error::at(file, "was not read"))
    }

    fn name(&self, file: &str) -> Result<&'static str, Error> {
        self.files.get_key_value(file).map(|(name, _)| *name).ok_or_else(|| Error::at(file, "was not read"))
    }

    fn bytes(&self, file: &str) -> Option<&[u8]> {
        self.now.get(file).and_then(Option::as_deref)
    }

    /// Whether `file` was there before this change.
    fn was(&self, file: &str) -> bool {
        self.files.get(file).is_some_and(|(_, bytes)| bytes.is_some())
    }

    /// Takes `file` to `next`, telling it as `change`.
    fn step(&mut self, change: Change, file: &str, next: Option<Vec<u8>>) -> Result<(), Error> {
        let (name, path) = (self.name(file)?, self.path(file)?);
        let before = self.now.get(name).and_then(Option::as_deref);
        if before == next.as_deref() {
            return Ok(());
        }
        self.steps.record(change, path, before, next.as_deref());
        self.now.insert(name, next);
        Ok(())
    }

    fn done(self, receipt: Receipt, notices: Vec<Notice>) -> Planned {
        Planned { files: self.now, receipt, notices, steps: self.steps }
    }
}

/// Connects `agent`: its server's entry for `server`, its shell off, its
/// limits raised, and `body` in its instructions. `previous`, the receipt
/// of its earlier connecting, lets what is still as written be written
/// anew; with `take_back`, what the user changed of Fastctx's is written as
/// this version writes it.
pub fn connect(
    agent: &Agent,
    files: &Files,
    previous: Option<&Receipt>,
    server: &Server<'_>,
    body: &str,
    take_back: bool,
) -> Result<Planned, Error> {
    let alone = previous.map_or_else(BTreeSet::new, |previous| fastctx_alone(agent, files, previous));
    let mut work = Working::new(files);
    let mut receipt = Receipt::default();
    let mut notices = Vec::new();

    let entry = (agent.mcp.entry)(server);
    let place = choose(&agent.mcp.to, files, &alone, &agent.mcp.slot)?;
    let held = previous.and_then(|previous| previous.server.as_ref());
    let before = moved(&mut work, Change::Tools, held, place, &mut notices)?;
    let owned = match agent.mcp.slot {
        // The agent may have copied Fastctx's entry to the file it reads now.
        Slot::Named if held.is_some() && before.is_none() => {
            match set(&mut work, place, "fastctx", &entry, None, take_back, false)? {
                (Owned::Set(placed), next) => (Owned::Set(placed.over_own_copy()), next),
                owned => owned,
            }
        }
        Slot::Named => set(&mut work, place, "fastctx", &entry, before, take_back, false)?,
        Slot::Listed { key, value } => {
            let ours = move |item: &Json| at_key(item, key).and_then(Json::as_str) == Some(value);
            add(&mut work, place, &entry, before, Some(&ours), take_back)?
        }
    };
    work.step(Change::Tools, place.file, Some(owned.1))?;
    receipt.server = Some(Held { file: place.file.to_owned(), owned: owned.0 });

    for (index, edit) in agent.shell.iter().enumerate() {
        let before = previous.and_then(|previous| previous.shell.get(index)).and_then(Option::as_ref);
        receipt.shell.push(apply_edit(&mut work, Change::Shell, edit, before, files, &alone, take_back, &mut notices)?);
    }
    for (index, edit) in agent.limits.iter().enumerate() {
        let before = previous.and_then(|previous| previous.limits.get(index)).and_then(Option::as_ref);
        receipt.limits.push(apply_edit(
            &mut work,
            Change::Limits,
            edit,
            before,
            files,
            &alone,
            take_back,
            &mut notices,
        )?);
    }
    if let Some(guidance) = agent.guidance {
        let file = guidance.file;
        let path = work.path(file)?;
        let found = work.bytes(file).map(<[u8]>::to_vec);
        let held = previous.and_then(|previous| previous.guidance.as_ref());
        // A file Fastctx makes starts with the agent's header.
        let starting = match (&found, guidance.head) {
            (None, Some(head)) if held.is_none_or(|held| !held.original_existed) => Some(head.as_bytes().to_vec()),
            _ => found.clone(),
        };
        let current = File { path, bytes: starting.as_deref() };
        let mut applied =
            if take_back { guidance::take_back(current, body, held)? } else { guidance::apply(current, body, held)? };
        if held.is_none() {
            applied.receipt.original_existed = work.was(file);
        }
        if guidance.most.is_some_and(|most| applied.bytes.len() > most) {
            // Past what the agent reads it would be cut off: left out, and an
            // earlier passage taken out with it.
            if let Some(held) = held {
                let taken = guidance::unapply(File { path, bytes: found.as_deref() }, Some(held))?;
                notices.extend(taken.retained);
                let bytes = headless(taken.bytes, guidance.head, held.original_existed);
                work.step(Change::Guidance, file, bytes)?;
            }
            notices.push(Notice {
                location: path.to_owned(),
                reason: "is longer than the agent reads; Fastctx's instructions are left out".into(),
            });
        } else {
            work.step(Change::Guidance, file, Some(applied.bytes))?;
            receipt.guidance = Some(applied.receipt);
        }
    }
    Ok(work.done(receipt, notices))
}

/// Whether something an earlier connecting put into `files`, as `receipt`
/// records it, is no longer there, no longer as written for the
/// instructions, or no longer where the agent reads it: connecting again
/// puts it back, taking back what someone took out
/// (`crate::apply::agent_refresh` keeps the files first).
pub fn taken_out(agent: &Agent, files: &Files, receipt: &Receipt) -> Result<bool, Error> {
    let alone = fastctx_alone(agent, files, receipt);
    let bytes = |file: &str| files.get(file).and_then(|(_, bytes)| bytes.as_deref());
    let path = |file: &str| files.get(file).map_or(file.to_owned(), |(path, _)| path.clone());
    let gone = |held: &Held, to: &To, slot: &Slot| -> Result<bool, Error> {
        if choose(to, files, &alone, slot)?.file != held.file {
            return Ok(true);
        }
        let (bytes, path) = (bytes(&held.file), path(&held.file));
        Ok(!match &held.owned {
            Owned::Set(placed) => json::holds_placed(bytes, &path, placed)?,
            Owned::Add(added) => json::holds_added(bytes, &path, added)?,
            Owned::Own(_) => bytes.is_some(),
            Owned::YamlSet(placed) => yaml::holds_placed(bytes, &path, placed)?,
            Owned::YamlAdd(added) => yaml::holds_added(bytes, &path, added)?,
        })
    };
    if let Some(held) = &receipt.server
        && gone(held, &agent.mcp.to, &agent.mcp.slot)?
    {
        return Ok(true);
    }
    let edits = agent.shell.iter().zip(&receipt.shell).chain(agent.limits.iter().zip(&receipt.limits));
    for (edit, held) in edits {
        let Some(held) = held else { continue };
        let to = match edit {
            Edit::Set { to, .. } | Edit::Raise { to, .. } | Edit::Default { to, .. } | Edit::Add { to, .. } => *to,
            Edit::Own { .. } => To::One(Place { file: edit.files()[0], format: Format::Json, at: &[] }),
        };
        if gone(held, &to, &Slot::Named)? {
            return Ok(true);
        }
    }
    if let (Some(guidance), Some(held)) = (agent.guidance, &receipt.guidance)
        && !guidance::holds(bytes(guidance.file), held)
    {
        return Ok(true);
    }
    Ok(false)
}

/// Takes `agent` out: what its receipt holds, where still as written,
/// last placed first.
pub fn disconnect(agent: &Agent, files: &Files, previous: &Receipt) -> Result<Planned, Error> {
    let mut work = Working::new(files);
    let mut notices = Vec::new();
    if let (Some(guidance), Some(held)) = (agent.guidance, &previous.guidance) {
        let path = work.path(guidance.file)?;
        let taken = guidance::unapply(File { path, bytes: work.bytes(guidance.file) }, Some(held))?;
        notices.extend(taken.retained);
        let bytes = headless(taken.bytes, guidance.head, held.original_existed);
        work.step(Change::Guidance, guidance.file, bytes)?;
    }
    for held in previous.limits.iter().rev().flatten() {
        notices.extend(undo(&mut work, Change::Limits, held)?);
    }
    for held in previous.shell.iter().rev().flatten() {
        notices.extend(undo(&mut work, Change::Shell, held)?);
    }
    if let Some(held) = &previous.server {
        let notice = undo(&mut work, Change::Tools, held)?;
        notices.extend(notice.or_else(|| left_listed(agent, &work, held)));
    }
    Ok(work.done(Receipt::default(), notices))
}

/// A server of Fastctx's kept in a list that the user changed since is
/// left there, and said so.
fn left_listed(agent: &Agent, work: &Working<'_>, held: &Held) -> Option<Notice> {
    let Slot::Listed { key, value } = agent.mcp.slot else { return None };
    let place = agent.mcp.to.places().iter().find(|place| place.file == held.file)?;
    let path = work.path(&held.file).ok()?;
    let bytes = work.bytes(&held.file);
    let settings = match place.format {
        Format::Json | Format::Jsonc => json::settings(bytes, path),
        Format::Yaml => yaml::read(bytes, path),
    }
    .ok()??;
    let servers = place.at.iter().try_fold(&settings, |value, step| value.get(*step))?.as_array()?;
    servers
        .iter()
        .any(|server| at_key(server, key).and_then(Json::as_str) == Some(value))
        .then(|| Notice { location: path.to_owned(), reason: "changed since Fastctx wrote it; left as it is".into() })
}

/// The files only Fastctx's places keep: taking the agent out would
/// delete them. Such a file does not count as there for `Pick::Present`.
fn fastctx_alone(agent: &Agent, files: &Files, receipt: &Receipt) -> BTreeSet<&'static str> {
    let Ok(taken) = disconnect(agent, files, receipt) else { return BTreeSet::new() };
    taken
        .files
        .iter()
        .filter(|(name, after)| after.is_none() && files.get(*name).is_some_and(|(_, bytes)| bytes.is_some()))
        .map(|(name, _)| *name)
        .collect()
}

/// The place of `to` the agent reads, as the files are found.
fn choose<'p>(to: &'p To, files: &Files, alone: &BTreeSet<&str>, slot: &Slot) -> Result<&'p Place, Error> {
    let (places, pick) = match to {
        To::One(place) => return Ok(place),
        To::Among(places, pick) => (*places, *pick),
    };
    let made = match pick {
        Pick::Present { made } => made,
        Pick::Holding => 0,
    };
    let first = places.get(made).ok_or_else(|| Error::at("an agent", "names no place for a change"))?;
    for place in places {
        let (path, bytes) =
            files.get(place.file).map_or((place.file, None), |(path, bytes)| (path.as_str(), bytes.as_deref()));
        let chosen = match pick {
            Pick::Present { .. } => bytes.is_some() && !alone.contains(place.file),
            Pick::Holding => servers_besides(place, path, bytes, slot)? > 0,
        };
        if chosen {
            return Ok(place);
        }
    }
    Ok(first)
}

/// How many servers besides Fastctx's `place` holds.
fn servers_besides(place: &Place, path: &str, bytes: Option<&[u8]>, slot: &Slot) -> Result<usize, Error> {
    let settings = match place.format {
        Format::Json | Format::Jsonc => json::settings(bytes, path)?,
        Format::Yaml => yaml::read(bytes, path)?,
    };
    let Some(settings) = settings else { return Ok(0) };
    let held = place.at.iter().try_fold(&settings, |value, key| value.get(*key));
    Ok(match (held, slot) {
        (Some(Json::Object(servers)), Slot::Named) => servers.keys().filter(|name| *name != "fastctx").count(),
        (Some(Json::Array(servers)), Slot::Listed { key, value }) => {
            servers.iter().filter(|server| at_key(server, key).and_then(Json::as_str) != Some(value)).count()
        }
        _ => 0,
    })
}

/// The value at `key` inside `value`, a number stepping into a list.
fn at_key<'v>(value: &'v Json, key: &[&str]) -> Option<&'v Json> {
    key.iter().try_fold(value, |value, step| match value {
        Json::Array(items) => step.parse::<usize>().ok().and_then(|index| items.get(index)),
        _ => value.get(*step),
    })
}

/// `before`, Fastctx's place written earlier, where it is still in `place`'s
/// file; one in another file is taken out of there first, as the agent
/// reads `place` now.
fn moved<'h>(
    work: &mut Working<'_>,
    change: Change,
    before: Option<&'h Held>,
    place: &Place,
    notices: &mut Vec<Notice>,
) -> Result<Option<&'h Held>, Error> {
    match before {
        Some(held) if held.file != place.file => {
            notices.extend(undo(work, change, held)?);
            Ok(None)
        }
        before => Ok(before),
    }
}

/// Sets `name` to `value` in the object at `place`.
fn set(
    work: &mut Working<'_>,
    place: &Place,
    name: &str,
    value: &Json,
    before: Option<&Held>,
    take_back: bool,
    adopt: bool,
) -> Result<(Owned, Vec<u8>), Error> {
    let path = work.path(place.file)?;
    let bytes = work.bytes(place.file);
    match place.format {
        Format::Json | Format::Jsonc => {
            let previous = before.and_then(|held| match &held.owned {
                Owned::Set(placed) => Some(placed),
                _ => None,
            });
            let (next, placed) = json::place(bytes, path, place.at, name, value, previous, take_back, adopt)?;
            let placed = if previous.is_none() && !work.was(place.file) { placed.in_made_file() } else { placed };
            Ok((Owned::Set(placed), next))
        }
        Format::Yaml => {
            let previous = before.and_then(|held| match &held.owned {
                Owned::YamlSet(placed) => Some(placed),
                _ => None,
            });
            let (next, placed) = yaml::place(bytes, path, place.at, name, value, previous, take_back, adopt)?;
            let placed = if previous.is_none() && !work.was(place.file) { placed.in_made_file() } else { placed };
            Ok((Owned::YamlSet(placed), next))
        }
    }
}

/// Adds `item` to the list at `place`; `ours` tells Fastctx's own item
/// where it is written anew.
fn add(
    work: &mut Working<'_>,
    place: &Place,
    item: &Json,
    before: Option<&Held>,
    ours: Option<&dyn Fn(&Json) -> bool>,
    take_back: bool,
) -> Result<(Owned, Vec<u8>), Error> {
    let path = work.path(place.file)?;
    let bytes = work.bytes(place.file);
    match place.format {
        Format::Json | Format::Jsonc => {
            if ours.is_some() {
                return Err(Error::at(path, "a list of servers is only kept in YAML"));
            }
            let previous = before.and_then(|held| match &held.owned {
                Owned::Add(added) => Some(added),
                _ => None,
            });
            let (next, added) = json::add(bytes, path, place.at, item, previous)?;
            let added = if previous.is_none() && !work.was(place.file) { added.in_made_file() } else { added };
            Ok((Owned::Add(added), next))
        }
        Format::Yaml => {
            let previous = before.and_then(|held| match &held.owned {
                Owned::YamlAdd(added) => Some(added),
                _ => None,
            });
            let (next, added) = yaml::add(bytes, path, place.at, item, previous, ours, take_back)?;
            let added = if previous.is_none() && !work.was(place.file) { added.in_made_file() } else { added };
            Ok((Owned::YamlAdd(added), next))
        }
    }
}

/// The value at `place` now.
fn current(work: &Working<'_>, place: &Place) -> Result<Option<Json>, Error> {
    let path = work.path(place.file)?;
    let bytes = work.bytes(place.file);
    match place.format {
        Format::Json | Format::Jsonc => json::value_at(bytes, path, place.at),
        Format::Yaml => yaml::value_at(bytes, path, place.at),
    }
}

/// Whether `current` is at least `wanted`, a negative number being no limit.
fn at_least(current: &Json, wanted: &Json) -> bool {
    match (current.as_f64(), wanted.as_f64()) {
        (Some(current), Some(wanted)) if wanted < 0.0 => current < 0.0,
        (Some(current), Some(wanted)) => current < 0.0 || current >= wanted,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_edit(
    work: &mut Working<'_>,
    change: Change,
    edit: &Edit,
    before: Option<&Held>,
    files: &Files,
    alone: &BTreeSet<&str>,
    take_back: bool,
    notices: &mut Vec<Notice>,
) -> Result<Option<Held>, Error> {
    if let Edit::Own { file, body } = edit {
        let previous = before.and_then(|held| match &held.owned {
            Owned::Own(owned) => Some(owned),
            _ => None,
        });
        let path = work.path(file)?;
        let (next, owned) = json::own(work.bytes(file), path, body, previous, take_back)?;
        work.step(change, file, Some(next))?;
        return Ok(Some(Held { file: (*file).to_owned(), owned: Owned::Own(owned) }));
    }
    let to = match edit {
        Edit::Set { to, .. } | Edit::Raise { to, .. } | Edit::Default { to, .. } | Edit::Add { to, .. } => to,
        Edit::Own { .. } => unreachable!("taken above"),
    };
    let place = choose(to, files, alone, &Slot::Named)?;
    let before = moved(work, change, before, place, notices)?;
    let (name, parent) = place.at.split_last().ok_or_else(|| Error::at(place.file, "a setting has no key"))?;
    let at = Place { at: parent, ..*place };
    let written = |held: &Held| match &held.owned {
        Owned::Set(placed) => Some(placed.written().clone()),
        Owned::YamlSet(placed) => Some(placed.written().clone()),
        _ => None,
    };
    let owned = match edit {
        Edit::Set { value, .. } => Some(set(work, &at, name, &value(), before, take_back, true)?),
        Edit::Raise { value, .. } => {
            let wanted = value();
            let now = current(work, place)?;
            let ours = before.is_some_and(|held| now.is_some() && now == written(held));
            match now {
                // As high already, and the user's: left to them.
                Some(now) if !ours && at_least(&now, &wanted) => None,
                _ => Some(set(work, &at, name, &wanted, before.filter(|_| ours), take_back, true)?),
            }
        }
        Edit::Default { value, .. } => {
            let wanted = value();
            match current(work, place)? {
                // There already: kept as it is, Fastctx's only while still as written.
                Some(now) => before
                    .filter(|held| written(held).as_ref() == Some(&now))
                    .map(|held| (held.owned.clone(), work.bytes(place.file).map(<[u8]>::to_vec).unwrap_or_default())),
                None => Some(set(work, &at, name, &wanted, None, take_back, false)?),
            }
        }
        Edit::Add { item, .. } => Some(add(work, place, &item(), before, None, take_back)?),
        Edit::Own { .. } => unreachable!("taken above"),
    };
    let Some((owned, next)) = owned else { return Ok(None) };
    work.step(change, place.file, Some(next))?;
    Ok(Some(Held { file: place.file.to_owned(), owned }))
}

/// Takes one of Fastctx's places out of its file, where still as written.
fn undo(work: &mut Working<'_>, change: Change, held: &Held) -> Result<Option<Notice>, Error> {
    let path = work.path(&held.file)?;
    let bytes = work.bytes(&held.file);
    let (next, notice) = match &held.owned {
        Owned::Set(placed) => json::unplace(bytes, path, placed)?,
        Owned::Add(added) => (json::unadd(bytes, path, added)?, None),
        Owned::Own(owned) => json::unown(bytes, path, owned),
        Owned::YamlSet(placed) => yaml::unplace(bytes, path, placed)?,
        Owned::YamlAdd(added) => yaml::unadd(bytes, path, added)?,
    };
    work.step(change, &held.file, next)?;
    Ok(notice)
}

/// A file Fastctx made with the agent's header goes when the header is all
/// that is left of it.
fn headless(bytes: Option<Vec<u8>>, head: Option<&str>, existed: bool) -> Option<Vec<u8>> {
    match (bytes, head) {
        (Some(bytes), Some(head)) if !existed && bytes == head.as_bytes() => None,
        (bytes, _) => bytes,
    }
}
