//! Bodies: what several accounts own together (design §6.2). A machine
//! shared whole is one body while it is in the directory — its owner and
//! the accounts it is shared with whole, as they are — and a conversation
//! or project opened on it is the body's, read by its members and its
//! opener. The hub keeps each body's members as the directory last had them:
//! once the machine leaves the directory (its owner deleted, the machine
//! forgotten), the members are frozen as they were, an account deleted since
//! counting for nothing, so those who shared it go on reading what they
//! shared. A body is its machine as it was: its key, its owner and when the
//! owner's account was opened.

use fastctx_net::directory::Directory;
use fastctx_storage::Database;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::MachineName;
use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, OnceLock, Weak};

/// The bodies, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS asset_bodies (
    id TEXT PRIMARY KEY, machine TEXT NOT NULL, members TEXT NOT NULL, revision INTEGER NOT NULL
) WITHOUT ROWID;
-- Owners none of whose scopes anyone reads any more, since when.
CREATE TABLE IF NOT EXISTS asset_unread (owner TEXT PRIMARY KEY, since INTEGER NOT NULL) WITHOUT ROWID;
-- Owners whose records went and whose files are still to go.
CREATE TABLE IF NOT EXISTS asset_deleting (owner TEXT PRIMARY KEY) WITHOUT ROWID;
";

/// An account as a body's member: its name and when it was opened.
fn member(directory: &Directory, account: &fastctx_types::AccountName) -> Option<String> {
    directory.account(account).map(|found| format!("{account}@{}", found.opened.as_millisecond()))
}

/// Whether `account`, as it is now, is among `members` of a body.
pub(crate) fn among(directory: &Directory, members: &BTreeSet<String>, account: &fastctx_types::AccountName) -> bool {
    member(directory, account).is_some_and(|account| members.contains(&account))
}

/// The body of `machine` as the directory has it now, if it has it.
pub(crate) fn body_of(directory: &Directory, machine: &MachineName) -> Option<String> {
    directory.body_of(machine)
}

/// The members of `machine`'s body as the directory has it now: its owner
/// and the accounts it is shared with whole, in force.
fn live(directory: &Directory, machine: &MachineName) -> BTreeSet<String> {
    let Some(found) = directory.member(machine) else { return BTreeSet::new() };
    let machine = directory.canonical(machine).clone();
    found
        .home
        .iter()
        .chain(directory.shares.iter().filter(|share| share.machine == machine && share.active()).map(|share| &share.account))
        .filter_map(|account| member(directory, account))
        .collect()
}

pub(crate) struct Bodies {
    control: Database,
    /// The hub, under whose directory writer a body's first members are
    /// taken: no save comes between reading them and keeping them (each save
    /// keeps them up to date in its own transaction after).
    hub: OnceLock<Weak<crate::Hub>>,
}

impl Bodies {
    pub(crate) fn new(control: Database) -> Self {
        Self { control, hub: OnceLock::new() }
    }

    /// The hub whose directory writer a body's first members are read under.
    pub(crate) fn serve(&self, hub: &Arc<crate::Hub>) {
        let _ = self.hub.set(Arc::downgrade(hub));
    }

    /// `work` done with the directory as it is, under its writer, so no save
    /// comes between what `work` judged by it and what it did; without a
    /// hub (the drills), with `directory`. Call on a thread that may block.
    pub(crate) fn under_directory<T>(&self, directory: &Directory, work: impl FnOnce(&Directory) -> T) -> T {
        match self.hub.get().and_then(Weak::upgrade) {
            Some(hub) => {
                let editing = hub.edit_store();
                work(&editing.book.directory)
            }
            None => work(directory),
        }
    }

    /// `body` of `machine` owns scopes from now: its members are kept, as
    /// the directory has them under its writer; without a hub (the drills),
    /// as `directory` has them.
    pub(crate) fn known(&self, directory: &Directory, body: &str, machine: &MachineName) -> io::Result<()> {
        let id = body.to_string();
        let kept = self
            .control
            .read_now(move |c| Ok(c.prepare_cached("SELECT 1 FROM asset_bodies WHERE id = ?1")?.query_row([&id], |_| Ok(())).optional()?))
            .map_err(io::Error::other)?;
        if kept.is_some() {
            return Ok(());
        }
        match self.hub.get().and_then(Weak::upgrade) {
            Some(hub) => {
                let editing = hub.edit_store();
                let machine = editing.book.directory.canonical(machine).clone();
                self.keep(&editing.book.directory, body, &machine)
            }
            None => self.keep(directory, body, machine),
        }
    }

    fn keep(&self, directory: &Directory, body: &str, machine: &MachineName) -> io::Result<()> {
        let members = serde_json::to_string(&live(directory, machine)).map_err(io::Error::other)?;
        let (body, machine) = (body.to_string(), machine.to_string());
        self.control
            .write_now(256 + members.len(), move |c| {
                c.prepare_cached("INSERT OR IGNORE INTO asset_bodies (id, machine, members, revision) VALUES (?1, ?2, ?3, 1)")?
                    .execute(params![body, machine, members])?;
                Ok(())
            })
            .map_err(io::Error::other)
    }

    /// The members to keep of each body whose machine `directory` has, as
    /// that body, where they differ from those kept: written in the
    /// transaction that saves `directory`, so a stop never leaves a saved
    /// directory with members it did not keep (§6.2).
    pub(crate) fn changes(&self, directory: &Directory) -> io::Result<Vec<(String, String)>> {
        let bodies: Vec<(String, String, String)> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT id, machine, members FROM asset_bodies")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let mut changes = Vec::new();
        for (id, machine, kept) in bodies {
            let Ok(name) = MachineName::try_from(machine) else { continue };
            if body_of(directory, &name).as_deref() != Some(id.as_str()) {
                continue;
            }
            let now = serde_json::to_string(&live(directory, &name)).map_err(io::Error::other)?;
            if now != kept {
                changes.push((id, now));
            }
        }
        Ok(changes)
    }

    /// Keeps `changes` (`changes`) on their own: as the hub opens, for a
    /// directory saved before members were kept with it.
    pub(crate) fn settle(&self, directory: &Directory) -> io::Result<()> {
        let changes = self.changes(directory)?;
        if changes.is_empty() {
            return Ok(());
        }
        self.control
            .write_now(256 * changes.len(), move |c| apply(c, &changes))
            .map_err(io::Error::other)
    }

    /// The members of `body` now: as the directory has them while its
    /// machine is in it, else as they were frozen; an account deleted or
    /// made again counts for nothing.
    pub(crate) fn members(&self, directory: &Directory, body: &str) -> io::Result<BTreeSet<String>> {
        let id = body.to_string();
        let kept: Option<(String, String)> = self
            .control
            .read_now(move |c| {
                Ok(c.prepare_cached("SELECT machine, members FROM asset_bodies WHERE id = ?1")?
                    .query_row([&id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?)
            })
            .map_err(io::Error::other)?;
        let Some((machine, members)) = kept else { return Ok(BTreeSet::new()) };
        if let Ok(name) = MachineName::try_from(machine)
            && body_of(directory, &name).as_deref() == Some(body)
        {
            return Ok(live(directory, &name));
        }
        let frozen: BTreeSet<String> = serde_json::from_str(&members).map_err(io::Error::other)?;
        let alive: BTreeSet<String> = directory.accounts.iter().map(|account| format!("{}@{}", account.name, account.opened.as_millisecond())).collect();
        Ok(frozen.intersection(&alive).cloned().collect())
    }
}

/// Writes bodies' members, in the transaction `c` is in.
pub(crate) fn apply(c: &fastctx_storage::rusqlite::Connection, changes: &[(String, String)]) -> fastctx_storage::Result<()> {
    for (id, members) in changes {
        c.prepare_cached("UPDATE asset_bodies SET members = ?2, revision = revision + 1 WHERE id = ?1")?.execute(params![id, members])?;
    }
    Ok(())
}
