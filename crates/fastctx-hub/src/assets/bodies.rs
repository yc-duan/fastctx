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
-- Each body the directory showed a machine shared whole as, kept from the
-- first save that showed it (§29, 2.1): its machine's name and device key,
-- and its members as they are or were frozen.
CREATE TABLE IF NOT EXISTS asset_bodies (
    id TEXT PRIMARY KEY, machine TEXT NOT NULL, members TEXT NOT NULL, revision INTEGER NOT NULL, key TEXT
) WITHOUT ROWID;
-- Each conversation described into a body, by its opener as opened (in
-- nanoseconds) and its resident: its records are read and written by the
-- body's readers, its opener's account gone or not (§29, 2.6).
CREATE TABLE IF NOT EXISTS asset_body_conversations (
    opener TEXT NOT NULL, opened TEXT NOT NULL, resident TEXT NOT NULL, body TEXT NOT NULL,
    PRIMARY KEY (opener, opened, resident)
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

/// A body's row as a directory save keeps it: its members where they
/// changed, or the whole row for a body the save shows first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BodyChange {
    pub(crate) id: String,
    pub(crate) machine: String,
    pub(crate) key: Option<String>,
    pub(crate) members: String,
}

/// Brings `asset_bodies` up to this release: the machine's key, kept from now.
pub(crate) fn migrate(c: &fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()> {
    let has_key = c.prepare("SELECT 1 FROM pragma_table_info('asset_bodies') WHERE name = 'key'")?.exists([])?;
    if !has_key {
        c.execute_batch("ALTER TABLE asset_bodies ADD COLUMN key TEXT")?;
    }
    Ok(())
}

/// A conversation by its opener, as opened, and its resident.
pub(crate) type ConversationOf = (fastctx_types::AccountName, jiff::Timestamp, String);

pub(crate) struct Bodies {
    control: Database,
    /// The conversations described into bodies, each with its body.
    conversations: std::sync::Mutex<std::collections::BTreeMap<ConversationOf, String>>,
    /// The hub, under whose directory writer a body's first members are
    /// taken: no save comes between reading them and keeping them (each save
    /// keeps them up to date in its own transaction after).
    hub: OnceLock<Weak<crate::Hub>>,
}

impl Bodies {
    pub(crate) fn new(control: Database) -> Self {
        Self { control, conversations: Default::default(), hub: OnceLock::new() }
    }

    /// Takes the conversations described into bodies as the hub keeps them.
    pub(crate) fn load_conversations(&self) -> io::Result<()> {
        let rows: Vec<(String, String, String, String)> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT opener, opened, resident, body FROM asset_body_conversations")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let mut kept = self.conversations.lock().expect("body conversations");
        for (opener, opened, resident, body) in rows {
            let (Ok(opener), Some(opened)) = (
                fastctx_types::AccountName::try_from(opener),
                opened.parse::<i128>().ok().and_then(|opened| jiff::Timestamp::from_nanosecond(opened).ok()),
            ) else {
                continue;
            };
            kept.insert((opener, opened, resident), body);
        }
        Ok(())
    }

    /// The body the conversation `of` was described into.
    pub(crate) fn conversation(&self, of: &ConversationOf) -> Option<String> {
        self.conversations.lock().expect("body conversations").get(of).cloned()
    }

    /// The conversations described into bodies, each with its body.
    pub(crate) fn conversations(&self) -> std::collections::BTreeMap<ConversationOf, String> {
        self.conversations.lock().expect("body conversations").clone()
    }

    /// Every opener, as opened, of a conversation described into a body.
    pub(crate) fn conversation_openers(&self) -> BTreeSet<(fastctx_types::AccountName, jiff::Timestamp)> {
        self.conversations.lock().expect("body conversations").keys().map(|(opener, opened, _)| (opener.clone(), *opened)).collect()
    }

    /// The opener `opener` as opened at the millisecond `at`, of a
    /// conversation described into a body.
    pub(crate) fn opener_at(&self, opener: &fastctx_types::AccountName, at: i64) -> Option<jiff::Timestamp> {
        self.conversations
            .lock()
            .expect("body conversations")
            .keys()
            .find(|(name, opened, _)| name == opener && opened.as_millisecond() == at)
            .map(|(_, opened, _)| *opened)
    }

    /// Whether `actor` is one of `body`'s members now (or as frozen), as the
    /// account it is: never one locked.
    pub(crate) fn member(&self, directory: &Directory, actor: &fastctx_types::AccountName, body: &str) -> bool {
        let Some(found) = directory.account(actor).filter(|found| !found.locked) else { return false };
        self.members(directory, body).is_ok_and(|members| members.contains(&format!("{actor}@{}", found.opened.as_millisecond())))
    }

    /// Keeps that the conversation `resident` `opener` opened at `opened` is
    /// `body`'s, in the transaction `c` is in.
    pub(crate) fn keep_conversation_in(
        c: &fastctx_storage::rusqlite::Connection,
        body: &str,
        opener: &fastctx_types::AccountName,
        opened: jiff::Timestamp,
        resident: &str,
    ) -> fastctx_storage::Result<()> {
        c.prepare_cached("INSERT OR IGNORE INTO asset_body_conversations (opener, opened, resident, body) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![opener.as_str(), opened.as_nanosecond().to_string(), resident, body])?;
        Ok(())
    }

    /// The same, as it is now kept, in memory.
    pub(crate) fn kept_conversation(&self, body: &str, opener: &fastctx_types::AccountName, opened: jiff::Timestamp, resident: &str) {
        self.conversations
            .lock()
            .expect("body conversations")
            .entry((opener.clone(), opened, resident.to_string()))
            .or_insert_with(|| body.to_string());
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
        let key = directory.member(machine).map(|member| member.key.to_string());
        let change = BodyChange { id: body.to_string(), machine: machine.to_string(), key, members };
        self.control.write_now(256 + change.members.len(), move |c| apply(c, &[change])).map_err(io::Error::other)
    }

    /// The members to keep of each body whose machine `directory` has, as
    /// that body, where they differ from those kept, and every body of a
    /// machine `directory` shows shared whole that is not kept yet (§29, 2.1):
    /// written in the transaction that saves `directory`, so a stop never
    /// leaves a saved directory with members it did not keep (§6.2).
    pub(crate) fn changes(&self, directory: &Directory) -> io::Result<Vec<BodyChange>> {
        let bodies: Vec<(String, String, String)> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT id, machine, members FROM asset_bodies")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let known: BTreeSet<String> = bodies.iter().map(|(id, _, _)| id.clone()).collect();
        let mut changes = Vec::new();
        for (id, machine, kept) in bodies {
            let Ok(name) = MachineName::try_from(machine.clone()) else { continue };
            if body_of(directory, &name).as_deref() != Some(id.as_str()) {
                continue;
            }
            let now = serde_json::to_string(&live(directory, &name)).map_err(io::Error::other)?;
            if now != kept {
                changes.push(BodyChange { id, machine, key: None, members: now });
            }
        }
        let shared: BTreeSet<&MachineName> = directory.shares.iter().filter(|share| share.active()).map(|share| &share.machine).collect();
        for member in directory.members.iter().filter(|member| shared.contains(&member.name)) {
            let Some(id) = body_of(directory, &member.name).filter(|id| !known.contains(id)) else { continue };
            let members = serde_json::to_string(&live(directory, &member.name)).map_err(io::Error::other)?;
            changes.push(BodyChange { id, machine: member.name.to_string(), key: Some(member.key.to_string()), members });
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

    /// Whether `machine`, as `directory` has it now, is the machine `body`
    /// was kept as: the body it is now, or one kept under its name and its
    /// device key (a key not kept, from before keys were, counts as its).
    pub(crate) fn is_of(&self, directory: &Directory, body: &str, machine: &MachineName) -> io::Result<bool> {
        if body_of(directory, machine).as_deref() == Some(body) {
            return Ok(true);
        }
        let Some(member) = directory.member(machine) else { return Ok(false) };
        let id = body.to_string();
        let kept: Option<(String, Option<String>)> = self
            .control
            .read_now(move |c| {
                Ok(c.prepare_cached("SELECT machine, key FROM asset_bodies WHERE id = ?1")?
                    .query_row([&id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?)
            })
            .map_err(io::Error::other)?;
        Ok(kept.is_some_and(|(name, key)| {
            name == directory.canonical(machine).as_str() && key.is_none_or(|key| key == member.key.to_string())
        }))
    }

    /// The machine `body` is, as kept when its first scope was described.
    pub(crate) fn machine(&self, body: &str) -> io::Result<Option<MachineName>> {
        let id = body.to_string();
        let machine: Option<String> = self
            .control
            .read_now(move |c| {
                Ok(c.prepare_cached("SELECT machine FROM asset_bodies WHERE id = ?1")?.query_row([&id], |row| row.get(0)).optional()?)
            })
            .map_err(io::Error::other)?;
        Ok(machine.and_then(|machine| MachineName::try_from(machine).ok()))
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
        members_now(directory, body, machine, &members)
    }

    /// Every body's members now, as `members` has each, read in one go: for
    /// what is reckoned of many accounts and machines at once. A body whose
    /// frozen members cannot be read is left out, as one that has none.
    pub(crate) fn all_members(&self, directory: &Directory) -> io::Result<Vec<(String, BTreeSet<String>)>> {
        let kept: Vec<(String, String, String)> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT id, machine, members FROM asset_bodies")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        Ok(kept
            .into_iter()
            .filter_map(|(body, machine, members)| {
                members_now(directory, &body, machine, &members).ok().map(|members| (body, members))
            })
            .collect())
    }
}

/// The members of `body`, kept as the machine `machine` with `members`
/// frozen: as the directory has them while its machine is in it, else as
/// they were frozen; an account deleted or made again counts for nothing.
fn members_now(directory: &Directory, body: &str, machine: String, members: &str) -> io::Result<BTreeSet<String>> {
    if let Ok(name) = MachineName::try_from(machine)
        && body_of(directory, &name).as_deref() == Some(body)
    {
        return Ok(live(directory, &name));
    }
    let frozen: BTreeSet<String> = serde_json::from_str(members).map_err(io::Error::other)?;
    let alive: BTreeSet<String> = directory.accounts.iter().map(|account| format!("{}@{}", account.name, account.opened.as_millisecond())).collect();
    Ok(frozen.intersection(&alive).cloned().collect())
}

/// Writes bodies' rows, in the transaction `c` is in: a body first shown
/// whole, members that changed; a key kept once stays.
pub(crate) fn apply(c: &fastctx_storage::rusqlite::Connection, changes: &[BodyChange]) -> fastctx_storage::Result<()> {
    for change in changes {
        c.prepare_cached(
            "INSERT INTO asset_bodies (id, machine, members, revision, key) VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT (id) DO UPDATE SET members = excluded.members, revision = asset_bodies.revision + 1,
             key = COALESCE(asset_bodies.key, excluded.key)",
        )?
        .execute(params![change.id, change.machine, change.members, change.key])?;
    }
    Ok(())
}
