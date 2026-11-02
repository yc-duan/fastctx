//! What the book keeps of a conversation deleted from the machine goes with it
//! (`world-engine.md`, 对话作为一个整体): its deeds, the files they touched,
//! all that hangs on them, and its counts by the hour; counts folded beyond
//! telling whose they are stay. The deletion seals the conversation here
//! first, for good, so that nothing told of it late is taken again
//! (`super::frozen`); its rows then go a few at a time with the book's
//! maintenance.
//!
//! A conversation is sealed by its id and its home: the home's name, folded
//! (`MachineName::key`), for one that lived here, under each name this
//! machine had; the room tag of its home for a room here of another
//! machine's conversation, or both for a room from before room tags.
//!
//! An account the hub deleted is sealed the same way, by its key (`hub/name`):
//! what was done for it before it went goes, as an account of the same name
//! opened since is another.

use std::collections::HashSet;
use std::io;

use fastctx_storage::{Database, Priority, rusqlite::Connection};
use fastctx_types::layout::{Kind, room_tag};
use fastctx_types::{MachineName, ResidentId};

use crate::Node;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS book_sealed (
    resident TEXT NOT NULL,
    place TEXT NOT NULL,
    purged INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (resident, place)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS book_hour_session ON book_hour(session);
";

pub(crate) const ACCOUNTS: &str = "
CREATE TABLE IF NOT EXISTS book_sealed_accounts (
    account TEXT PRIMARY KEY,
    before INTEGER NOT NULL,
    purged INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS book_deeds_account ON book_deeds(account, began);
CREATE INDEX IF NOT EXISTS book_hour_account ON book_hour(account, at);
";

/// The tables whose rows hang on a deed, by its key first.
const BY_DEED: [&str; 9] = [
    "book_deeds",
    "book_touches",
    "book_steps",
    "book_writes",
    "book_visits",
    "book_cursors",
    "book_complete",
    "book_folding",
    "book_retired",
];

/// How many deeds, or hours, go in one transaction.
const BATCH: usize = 128;

/// The places a conversation whose home is `machine` is sealed under.
fn places(machine: &MachineName) -> [String; 2] {
    [format!("name:{}", machine.key()), format!("room:{}", room_tag(machine))]
}

/// The conversation kept here as `name`, and the places it is sealed under.
fn sealing(node: &Node, name: &str) -> Option<(ResidentId, Vec<String>)> {
    match Kind::of(name)? {
        Kind::Home(id) => {
            let names = node.machine_names();
            let places =
                std::iter::once(names.name).chain(names.was).map(|machine| format!("name:{}", machine.key())).collect();
            Some((id, places))
        }
        Kind::Visiting { resident, home } => Some((resident, vec![format!("room:{home}")])),
        Kind::Guest(owner) => Some((owner.resident.clone(), places(&owner.machine).to_vec())),
        Kind::Unplaced(_) => None,
    }
}

/// Seals the conversation kept here as `name`, which is being deleted. While
/// the book is unavailable none of it can be read or written; the node seals
/// it when it next starts (`reconcile`).
pub(crate) fn seal(node: &Node, name: &str) -> io::Result<()> {
    match sealing(node, name) {
        Some((resident, places)) => keep(node, vec![(resident.to_string(), places)]),
        None => Ok(()),
    }
}

/// Seals the conversation `resident` whose home answers to `machines`, told
/// of its deletion by its hub: its activity here goes whether or not a room
/// of it is still here.
pub(crate) fn seal_elsewhere(node: &Node, resident: &ResidentId, machines: &[MachineName]) -> io::Result<()> {
    let places = machines.iter().flat_map(places).collect();
    keep(node, vec![(resident.to_string(), places)])
}

fn keep(node: &Node, seals: Vec<(String, Vec<String>)>) -> io::Result<()> {
    if node.book_database.fault().is_some() {
        return Ok(());
    }
    let bytes = seals.iter().map(|(resident, places)| resident.len() + places.iter().map(String::len).sum::<usize>());
    node.book_database
        .write_now_at(Priority::Control, bytes.sum::<usize>() + 64, move |db| {
            for (resident, places) in &seals {
                for place in places {
                    db.execute(
                        "INSERT OR IGNORE INTO book_sealed(resident, place) VALUES (?1, ?2)",
                        (resident, place),
                    )?;
                }
            }
            Ok(())
        })
        .map_err(io::Error::other)
}

/// Seals what was done here for `account` of the hub `hub`, deleted at
/// `at`, before it went. With the book unavailable it is not done, and the hub
/// asks again.
pub(crate) fn seal_account(
    node: &Node,
    hub: &impl std::fmt::Display,
    account: &fastctx_types::AccountName,
    at: jiff::Timestamp,
) -> io::Result<()> {
    if let Some(error) = node.book_database.fault() {
        return Err(io::Error::other(format!("the activity book is unavailable: {error}")));
    }
    let (account, before) = (format!("{hub}/{account}"), at.as_millisecond());
    node.book_database
        .write_now_at(Priority::Control, account.len() + 64, move |db| {
            db.execute(
                "INSERT INTO book_sealed_accounts(account, before) VALUES (?1, ?2)
                 ON CONFLICT(account) DO UPDATE SET before=max(before, excluded.before), purged=0",
                (account, before),
            )?;
            Ok(())
        })
        .map_err(io::Error::other)
}

/// Whether what was done for `account` beginning at `began` is of an account
/// deleted since.
pub(super) fn account_sealed(db: &Connection, account: &str, began: i64) -> fastctx_storage::Result<bool> {
    if account.is_empty() {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM book_sealed_accounts WHERE account=?1 AND ?2<before)",
        (account, began),
        |row| row.get(0),
    )?)
}

/// Whether the deed keyed `deed` was done for an account deleted since.
pub(super) fn deed_account_sealed(db: &Connection, deed: &str) -> fastctx_storage::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM book_deeds d JOIN book_sealed_accounts s ON s.account=d.account
         WHERE d.deed=?1 AND d.began<s.before)",
        [deed],
        |row| row.get(0),
    )?)
}

/// Seals each conversation the node's records say was deleted, a page at a
/// time: one deleted while the book was unavailable, or before it sealed.
pub(crate) fn reconcile(node: &Node) -> io::Result<()> {
    if node.book_database.fault().is_some() || node.database.fault().is_some() {
        return Ok(());
    }
    let mut after = String::new();
    loop {
        let from = after.clone();
        let names: Vec<String> = node
            .database
            .read_background_now(move |db| {
                Ok(db
                    .prepare("SELECT name FROM node_conversation_deletions WHERE name>?1 ORDER BY name LIMIT 256")?
                    .query_map([from], |row| row.get(0))?
                    .collect::<Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let Some(last) = names.last().cloned() else { break };
        keep(
            node,
            names.iter().filter_map(|name| sealing(node, name)).map(|(id, places)| (id.to_string(), places)).collect(),
        )?;
        after = last;
    }
    let tombstones: Vec<_> = node.residents.tombstones().into_iter().map(|(id, _)| id).collect();
    for page in tombstones.chunks(256) {
        keep(
            node,
            page.iter()
                .filter_map(|id| sealing(node, id.as_str()))
                .map(|(id, places)| (id.to_string(), places))
                .collect(),
        )?;
    }
    Ok(())
}

/// Whether the deed keyed `deed` is a job of a sealed conversation.
pub(super) fn sealed(db: &Connection, deed: &str) -> fastctx_storage::Result<bool> {
    let Some((resident, machine)) = owner(deed) else { return Ok(false) };
    let [name, room] = places(&machine);
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM book_sealed WHERE resident=?1 AND place IN (?2, ?3))",
        (resident.as_str(), name, room),
        |row| row.get(0),
    )?)
}

/// The conversation and its home a deed's key names (`super::deed`): its
/// owner as an address (`codex-1a2b3c4d on desktop`), then its job and when
/// it began. None for a page's.
fn owner(deed: &str) -> Option<(ResidentId, MachineName)> {
    let mut parts = deed.split(' ');
    let resident = ResidentId::try_from(parts.next()?).ok()?;
    if parts.next()? != "on" {
        return None;
    }
    let machine = MachineName::try_from(parts.next()?).ok()?;
    Some((resident, machine))
}

/// Lets go of the rows of the conversations sealed here, a few deeds at a
/// time; the seals stay.
pub(crate) fn purge(database: &Database) -> io::Result<()> {
    loop {
        let seals: Vec<(String, String)> = database
            .read_background_now(|db| {
                Ok(db
                    .prepare("SELECT resident, place FROM book_sealed WHERE purged=0 LIMIT 16")?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        if seals.is_empty() {
            return accounts(database);
        }
        for (resident, place) in seals {
            while deeds(database, &resident, &place)? {}
            while hours(database, &resident, &place)? {}
            database
                .write_now_at(Priority::Background, 128, move |db| {
                    db.execute("UPDATE book_sealed SET purged=1 WHERE resident=?1 AND place=?2", (resident, place))?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
        }
    }
}

/// Lets go of what was done for the sealed accounts before they went, a few
/// deeds and hours at a time; the seals stay.
fn accounts(database: &Database) -> io::Result<()> {
    loop {
        let seals: Vec<(String, i64)> = database
            .read_background_now(|db| {
                Ok(db
                    .prepare("SELECT account, before FROM book_sealed_accounts WHERE purged=0 LIMIT 16")?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        if seals.is_empty() {
            return Ok(());
        }
        for (account, before) in seals {
            loop {
                let wanted = account.clone();
                let deeds: Vec<String> = database
                    .read_background_now(move |db| {
                        Ok(db
                            .prepare("SELECT deed FROM book_deeds WHERE account=?1 AND began<?2 LIMIT ?3")?
                            .query_map((wanted, before, BATCH as i64), |row| row.get(0))?
                            .collect::<Result<_, _>>()?)
                    })
                    .map_err(io::Error::other)?;
                if deeds.is_empty() {
                    break;
                }
                let bytes = deeds.iter().map(String::len).sum::<usize>() * BY_DEED.len();
                database
                    .write_now_at(Priority::Background, bytes, move |db| {
                        for deed in &deeds {
                            for table in BY_DEED {
                                db.execute(&format!("DELETE FROM {table} WHERE deed=?1"), [deed])?;
                            }
                        }
                        Ok(())
                    })
                    .map_err(io::Error::other)?;
            }
            loop {
                let wanted = account.clone();
                let went = database
                    .write_now_at(Priority::Background, 256, move |db| {
                        Ok(db.execute(
                            "DELETE FROM book_hour WHERE (at, session, agent, held, account) IN
                                (SELECT at, session, agent, held, account FROM book_hour
                                 WHERE account=?1 AND at<?2 LIMIT ?3)",
                            (wanted, before, BATCH as i64),
                        )?)
                    })
                    .map_err(io::Error::other)?;
                if went == 0 {
                    break;
                }
            }
            database
                .write_now_at(Priority::Background, 128, move |db| {
                    db.execute(
                        "UPDATE book_sealed_accounts SET purged=1 WHERE account=?1 AND before=?2",
                        (account, before),
                    )?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
        }
    }
}

/// Removes up to `BATCH` deeds of the sealed conversation, with what hangs on
/// them; says whether any went.
fn deeds(database: &Database, resident: &str, place: &str) -> io::Result<bool> {
    // A deed's key begins with its owner's address: `<id> on <machine>`.
    let (low, high) = (format!("{resident} on "), format!("{resident} on!"));
    let wanted = place.to_string();
    let found: Vec<String> = database
        .read_background_now(move |db| {
            let mut found = HashSet::new();
            for table in BY_DEED {
                let mut query = db.prepare(&format!(
                    "SELECT DISTINCT deed FROM {table} WHERE deed>?1 AND deed<?2 ORDER BY deed LIMIT 1024"
                ))?;
                // Past the keys of another machine's conversation of the same
                // id, a page at a time.
                let mut after = low.clone();
                loop {
                    let keys: Vec<String> =
                        query.query_map((&after, &high), |row| row.get::<_, String>(0))?.collect::<Result<_, _>>()?;
                    let Some(last) = keys.last().cloned() else { break };
                    for key in keys {
                        if owner(&key).is_some_and(|(_, machine)| places(&machine).contains(&wanted)) {
                            found.insert(key);
                        }
                    }
                    if found.len() >= BATCH {
                        return Ok(found.into_iter().collect());
                    }
                    after = last;
                }
            }
            Ok(found.into_iter().collect())
        })
        .map_err(io::Error::other)?;
    if found.is_empty() {
        return Ok(false);
    }
    let bytes = found.iter().map(String::len).sum::<usize>() * BY_DEED.len();
    database
        .write_now_at(Priority::Background, bytes, move |db| {
            for deed in &found {
                for table in BY_DEED {
                    db.execute(&format!("DELETE FROM {table} WHERE deed=?1"), [deed])?;
                }
            }
            Ok(())
        })
        .map_err(io::Error::other)?;
    Ok(true)
}

/// Removes up to `BATCH` of the sealed conversation's counts by the hour;
/// says whether any went.
fn hours(database: &Database, resident: &str, place: &str) -> io::Result<bool> {
    // A session is `<id>@<machine>`; `A` follows `@`.
    let (low, high) = (format!("{resident}@"), format!("{resident}A"));
    let wanted = place.to_string();
    let sessions: Vec<String> = database
        .read_background_now(move |db| {
            let mut query = db.prepare("SELECT DISTINCT session FROM book_hour WHERE session>=?1 AND session<?2")?;
            let sessions = query.query_map((&low, &high), |row| row.get::<_, String>(0))?;
            let mut found = Vec::new();
            for session in sessions {
                let session = session?;
                let machine = session.split_once('@').and_then(|(_, machine)| MachineName::try_from(machine).ok());
                if machine.is_some_and(|machine| places(&machine).contains(&wanted)) {
                    found.push(session);
                }
            }
            Ok(found)
        })
        .map_err(io::Error::other)?;
    let mut removed = false;
    for session in sessions {
        let went = database
            .write_now_at(Priority::Background, 256, move |db| {
                Ok(db.execute(
                    "DELETE FROM book_hour WHERE (at, session, agent, held, account) IN
                        (SELECT at, session, agent, held, account FROM book_hour WHERE session=?1 LIMIT ?2)",
                    (session, BATCH as i64),
                )?)
            })
            .map_err(io::Error::other)?;
        removed |= went > 0;
    }
    Ok(removed)
}
