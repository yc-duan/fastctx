//! Working out what connecting an agent, or taking it out, changes, with
//! nothing read or written: the files as they are in, each file's bytes
//! after and the receipt out, with the steps the window before the change
//! tells (`world-layers.md`, 接入之前讲清楚做了什么). Its server's entry
//! comes first, then its shell, its limits, and the passage in its
//! instructions, each told as its own kind of change.

use std::collections::BTreeMap;

use fastctx_types::logins::Change;
use serde::{Deserialize, Serialize};

use super::json::{self, Added, OwnFile, Placed};
use super::spec::{Agent, Edit, Server};
use crate::connect::Steps;
use crate::{Error, File, Notice, guidance};

/// What connecting an agent wrote, to take out again what is still as it
/// was written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server: Option<Placed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shell: Option<Owned>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    limits: Vec<Option<Owned>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guidance: Option<guidance::Receipt>,
}

impl Receipt {
    /// Whether anything of Fastctx's is held: the agent is connected.
    pub fn holds(&self) -> bool {
        self.server.is_some()
            || self.shell.is_some()
            || self.limits.iter().any(Option::is_some)
            || self.guidance.is_some()
    }
}

/// One edit's part of the receipt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Owned {
    Set(Placed),
    Add(Added),
    Own(OwnFile),
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

    fn path(&self, file: &'static str) -> Result<&'a str, Error> {
        self.files.get(file).map(|(path, _)| path.as_str()).ok_or_else(|| Error::at(file, "was not read"))
    }

    fn bytes(&self, file: &'static str) -> Option<&[u8]> {
        self.now.get(file).and_then(Option::as_deref)
    }

    /// Takes `file` to `next`, telling it as `change`.
    fn step(&mut self, change: Change, file: &'static str, next: Option<Vec<u8>>) -> Result<(), Error> {
        let path = self.path(file)?;
        let before = self.now.get(file).and_then(Option::as_deref);
        self.steps.record(change, path, before, next.as_deref());
        self.now.insert(file, next);
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
    let mut work = Working::new(files);
    let mut receipt = Receipt::default();
    let notices = Vec::new();
    let entry = (agent.mcp.entry)(server);
    let file = agent.mcp.file;
    let (next, placed) = json::place(
        work.bytes(file),
        work.path(file)?,
        agent.mcp.at,
        "fastctx",
        &entry,
        previous.and_then(|previous| previous.server.as_ref()),
        take_back,
        false,
    )?;
    work.step(Change::Tools, file, Some(next))?;
    receipt.server = Some(placed);

    if let Some(edit) = &agent.shell {
        let before = previous.and_then(|previous| previous.shell.as_ref());
        receipt.shell = Some(apply_edit(&mut work, Change::Shell, edit, before, take_back)?);
    }
    for (index, edit) in agent.limits.iter().enumerate() {
        let before = previous.and_then(|previous| previous.limits.get(index)).and_then(Option::as_ref);
        let owned = apply_edit(&mut work, Change::Limits, edit, before, take_back)?;
        receipt.limits.push(Some(owned));
    }
    if let Some(file) = agent.guidance {
        let path = work.path(file)?;
        let current = File { path, bytes: work.bytes(file) };
        let previous = previous.and_then(|previous| previous.guidance.as_ref());
        let applied = if take_back {
            guidance::take_back(current, body, previous)?
        } else {
            guidance::apply(current, body, previous)?
        };
        work.step(Change::Guidance, file, Some(applied.bytes))?;
        receipt.guidance = Some(applied.receipt);
    }
    Ok(work.done(receipt, notices))
}

/// Whether something an earlier connecting put into `files`, as `receipt`
/// records it, is no longer there, or no longer as written for the
/// instructions: connecting again puts it back, taking back what someone
/// took out (`crate::apply::agent_refresh` keeps the files first).
pub fn taken_out(agent: &Agent, files: &Files, receipt: &Receipt) -> Result<bool, Error> {
    let bytes = |file: &'static str| files.get(file).and_then(|(_, bytes)| bytes.as_deref());
    let path = |file: &'static str| files.get(file).map_or(file, |(path, _)| path.as_str());
    if let Some(placed) = &receipt.server
        && !json::holds_placed(bytes(agent.mcp.file), path(agent.mcp.file), placed)?
    {
        return Ok(true);
    }
    let edits = agent.shell.iter().zip(&receipt.shell).chain(
        agent.limits.iter().zip(&receipt.limits).filter_map(|(edit, owned)| owned.as_ref().map(|owned| (edit, owned))),
    );
    for (edit, owned) in edits {
        let file = edit.file();
        let held = match owned {
            Owned::Set(placed) => json::holds_placed(bytes(file), path(file), placed)?,
            Owned::Add(added) => json::holds_added(bytes(file), path(file), added)?,
            Owned::Own(_) => bytes(file).is_some(),
        };
        if !held {
            return Ok(true);
        }
    }
    if let (Some(file), Some(held)) = (agent.guidance, &receipt.guidance)
        && !guidance::holds(bytes(file), held)
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
    if let (Some(file), Some(held)) = (agent.guidance, &previous.guidance) {
        let path = work.path(file)?;
        let taken = guidance::unapply(File { path, bytes: work.bytes(file) }, Some(held))?;
        notices.extend(taken.retained);
        work.step(Change::Guidance, file, taken.bytes)?;
    }
    for (edit, held) in agent.limits.iter().zip(&previous.limits).rev() {
        if let Some(held) = held {
            notices.extend(undo_edit(&mut work, Change::Limits, edit, held)?);
        }
    }
    if let (Some(edit), Some(held)) = (&agent.shell, &previous.shell) {
        notices.extend(undo_edit(&mut work, Change::Shell, edit, held)?);
    }
    if let Some(placed) = &previous.server {
        let file = agent.mcp.file;
        let (next, notice) = json::unplace(work.bytes(file), work.path(file)?, placed)?;
        notices.extend(notice);
        work.step(Change::Tools, file, next)?;
    }
    Ok(work.done(Receipt::default(), notices))
}

fn apply_edit(
    work: &mut Working<'_>,
    change: Change,
    edit: &Edit,
    before: Option<&Owned>,
    take_back: bool,
) -> Result<Owned, Error> {
    let file = edit.file();
    let path = work.path(file)?;
    match edit {
        Edit::Set { at, value, .. } => {
            let (name, parent) = at.split_last().ok_or_else(|| Error::at(path, "a setting has no key"))?;
            let previous = match before {
                Some(Owned::Set(placed)) => Some(placed),
                _ => None,
            };
            let (next, placed) =
                json::place(work.bytes(file), path, parent, name, &value(), previous, take_back, true)?;
            work.step(change, file, Some(next))?;
            Ok(Owned::Set(placed))
        }
        Edit::Add { at, item, .. } => {
            let previous = match before {
                Some(Owned::Add(added)) => Some(added),
                _ => None,
            };
            let (next, added) = json::add(work.bytes(file), path, at, &item(), previous)?;
            work.step(change, file, Some(next))?;
            Ok(Owned::Add(added))
        }
        Edit::Own { body, .. } => {
            let previous = match before {
                Some(Owned::Own(owned)) => Some(owned),
                _ => None,
            };
            let (next, owned) = json::own(work.bytes(file), path, body, previous, take_back)?;
            work.step(change, file, Some(next))?;
            Ok(Owned::Own(owned))
        }
    }
}

fn undo_edit(work: &mut Working<'_>, change: Change, edit: &Edit, held: &Owned) -> Result<Option<Notice>, Error> {
    let file = edit.file();
    let path = work.path(file)?;
    let (next, notice) = match held {
        Owned::Set(placed) => json::unplace(work.bytes(file), path, placed)?,
        Owned::Add(added) => (json::unadd(work.bytes(file), path, added)?, None),
        Owned::Own(owned) => json::unown(work.bytes(file), path, owned),
    };
    work.step(change, file, next)?;
    Ok(notice)
}
