//! Machine-log indexes preserve the readers fixed when a change was made.
//! Account incarnation and device key are part of every visibility lookup.

use super::shelf;
use crate::machine_log::{Kept, notification};
use fastctx_storage::{
    Database, Error,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::{
    AccountName,
    machine_log::{Action, Needs, Page, Reads, Record},
};
use jiff::Timestamp;

pub(crate) struct Row {
    pub(crate) id: u64,
    pub(crate) at: Timestamp,
    pub(crate) record: Vec<u8>,
    readers: Vec<String>,
    notified: Vec<String>,
    needs: i64,
    updated: Vec<(String, String, String)>,
}
impl Row {
    pub(crate) fn from_bytes(record: Vec<u8>) -> fastctx_storage::Result<Self> {
        let kept: Kept = shelf::decode(&record)?;
        let mut readers: Vec<_> = kept.accounts.iter().map(|(name, opened)| account(name, *opened)).collect();
        readers.extend(kept.members.iter().map(|key| format!("member:{key}")));
        if !kept.record.action.of_conversation() && !kept.record.action.of_project() {
            readers.push("whole".into());
        }
        let notified = kept.notified.iter().map(|(name, opened)| account(name, *opened)).collect();
        let updated = match &kept.record.action {
            Action::Updated { from, to, .. } => {
                kept.members.iter().map(|key| (key.to_string(), from.clone(), to.clone())).collect()
            }
            _ => Vec::new(),
        };
        Ok(Self {
            id: kept.record.id,
            at: kept.record.at,
            record,
            readers,
            notified,
            needs: need(kept.record.action.needs()),
            updated,
        })
    }
    pub(crate) fn bytes(&self) -> usize {
        self.record.len() + self.readers.iter().chain(&self.notified).map(String::len).sum::<usize>() + 128
    }
}

pub(crate) fn account(name: &AccountName, opened: Timestamp) -> String {
    format!("account:{name}:{opened}")
}
fn need(needs: Needs) -> i64 {
    match needs {
        Needs::Nothing => 0,
        Needs::Renames => 1,
        Needs::Forgets => 2,
        Needs::Versions => 4,
        Needs::Due => 8,
        Needs::ProjectDeletes => 16,
        Needs::Profiles => 32,
        Needs::MachineUpdates => 64,
        Needs::HistorySwitch => 128,
    }
}
fn mask(reads: Reads) -> i64 {
    [
        Needs::Renames,
        Needs::Forgets,
        Needs::Versions,
        Needs::Due,
        Needs::ProjectDeletes,
        Needs::Profiles,
        Needs::MachineUpdates,
        Needs::HistorySwitch,
    ]
    .into_iter()
    .filter(|n| reads.covers(*n))
    .map(need)
    .sum()
}

/// Maps the complete unsigned log sequence into SQLite's signed order.
pub(crate) fn sql_id(id: u64) -> i64 {
    (id ^ (1_u64 << 63)) as i64
}

pub(crate) fn put(c: &Connection, row: &Row) -> fastctx_storage::Result<()> {
    let id = sql_id(row.id);
    // Replacing an uncertain write also replaces its visibility indexes.
    c.prepare_cached("DELETE FROM machine_log WHERE id=?1")?.execute([id])?;
    c.prepare_cached("INSERT INTO machine_log(id,at,nano,needs,record) VALUES(?1,?2,?3,?4,?5)")?.execute(params![
        id,
        row.at.as_second(),
        row.at.subsec_nanosecond(),
        row.needs,
        row.record
    ])?;
    for reader in &row.readers {
        c.prepare_cached("INSERT OR IGNORE INTO machine_log_readers(reader,id) VALUES(?1,?2)")?
            .execute(params![reader, id])?;
    }
    for reader in &row.notified {
        c.prepare_cached("INSERT OR IGNORE INTO machine_log_notified(reader,id) VALUES(?1,?2)")?
            .execute(params![reader, id])?;
    }
    for (machine, was, now) in &row.updated {
        c.prepare_cached("INSERT INTO machine_log_updated(machine,was,now,id) VALUES(?1,?2,?3,?4)")?
            .execute(params![machine, was, now, id])?;
    }
    Ok(())
}

/// The number of the newest record kept, if any.
pub(crate) fn newest(database: &Database) -> std::io::Result<Option<u64>> {
    database
        .read_now(|c| {
            let id: Option<i64> =
                c.prepare_cached("SELECT id FROM machine_log ORDER BY id DESC LIMIT 1")?.query_row([], |r| r.get(0)).optional()?;
            Ok(id.map(|id| (id as u64) ^ (1_u64 << 63)))
        })
        .map_err(shelf::other)
}

/// The records any of `readers` reads numbered after `after`, oldest first,
/// at most `limit`: each by its number, as its JSON.
pub(crate) fn readable_after(
    database: &Database,
    readers: &[String],
    after: Option<u64>,
    limit: usize,
) -> std::io::Result<Vec<(u64, Vec<u8>)>> {
    let readers = readers.to_vec();
    database
        .read_now(move |c| {
            let from = after.map_or(i64::MIN, sql_id);
            let mut found = std::collections::BTreeMap::new();
            let mut stmt = c.prepare_cached(
                "SELECT r.id,l.record FROM machine_log_readers r JOIN machine_log l ON l.id=r.id WHERE r.reader=?1 AND r.id>?2 ORDER BY r.id LIMIT ?3",
            )?;
            for reader in &readers {
                for row in stmt.query_map(params![reader, from, limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
                    let (id, record) = row?;
                    found.entry(id).or_insert(record);
                }
            }
            let mut rows = Vec::new();
            for (id, record) in found.into_iter().take(limit) {
                let record = shelf::decode::<VisibleRecord>(&record)?.record;
                let body = serde_json::to_vec(&record).map_err(|e| Error::new(e.to_string()))?;
                rows.push(((id as u64) ^ (1_u64 << 63), body));
            }
            Ok(rows)
        })
        .map_err(shelf::other)
}

pub(crate) fn updated(
    database: &Database,
    key: &fastctx_net::sign::PublicKey,
    was: &str,
    now: &str,
) -> std::io::Result<bool> {
    database
        .read_now(|c| {
            Ok(c.prepare_cached("SELECT 1 FROM machine_log_updated WHERE machine=?1 AND was=?2 AND now=?3 LIMIT 1")?
                .query_row(params![key.to_string(), was, now], |_| Ok(()))
                .optional()?
                .is_some())
        })
        .map_err(shelf::other)
}

#[derive(serde::Deserialize)]
struct VisibleRecord {
    record: Record,
}

/// Each reader contributes only one page of candidates, so the merge never
/// sorts the entire history of an account that has millions of records.
pub(crate) fn read(
    database: &Database,
    readers: &[String],
    before: Option<u64>,
    reads: Reads,
) -> Result<Page, String> {
    database.read_now(|c| {
        let mut page = Page {records:Vec::new(),next_before:None};
        let envelope = fastctx_net::wire::Down::Told {id:u64::MAX,answer:Ok(fastctx_net::wire::Told::MachineLog {page:Page {records:Vec::new(),next_before:Some(u64::MAX)}})};
        let mut bytes = fastctx_net::wire::encode(&envelope).len();
        let mut cursor = match before {
            Some(0) => return Ok(page),
            Some(before) => sql_id(before - 1),
            None => i64::MAX,
        };
        loop {
            let mut records = std::collections::BTreeMap::new();
            let mut more = false;
            for reader in readers {
                let mut stmt = c.prepare_cached("SELECT r.id,l.record FROM machine_log_readers r JOIN machine_log l ON l.id=r.id WHERE reader=?1 AND r.id<=?2 AND (l.needs & ?3)=l.needs ORDER BY r.id DESC LIMIT 256")?;
                let mut count = 0;
                for row in stmt.query_map(params![reader,cursor,mask(reads)],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?)))? {
                    let (id, record) = row?;
                    records.entry(id).or_insert(record);
                    count += 1;
                }
                more |= count == 256;
            }
            more |= records.len() > 256;
            if records.is_empty() { break; }
            for (id, record) in records.into_iter().rev().take(256) {
                let record = shelf::decode::<VisibleRecord>(&record)?.record;
                let added = fastctx_net::wire::encode(&record).len()+usize::from(!page.records.is_empty());
                if bytes+added > fastctx_net::wire::MAX_CONTROL_FRAME {
                    let last = page.records.last().ok_or_else(||Error::new("one machine log record exceeds a control frame"))?;
                    page.next_before = Some(last.id);
                    return Ok(page);
                }
                bytes += added;
                page.records.push(record);
                let Some(previous) = id.checked_sub(1) else { return Ok(page); };
                cursor = previous;
            }
            if !more { break; }
        }
        Ok(page)
    }).map_err(|e|e.to_string())
}

pub(crate) fn notices(
    database: &Database,
    accounts: &[(AccountName, Timestamp)],
) -> std::io::Result<Vec<fastctx_types::sharing::Notice>> {
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    database.read_now(|c| {
        // The newest of each reader's, read back along its own index and no
        // further than the most a view carries: a long history costs nothing
        // here. Their newest together, then the records still kept.
        let most = crate::machine_log::NOTICES;
        let mut newest = c.prepare_cached("SELECT id FROM machine_log_notified WHERE reader = ?1 ORDER BY id DESC LIMIT ?2")?;
        let mut ids = std::collections::BTreeSet::new();
        for (name, opened) in accounts {
            let found = newest.query_map(params![account(name, *opened), most as i64], |row| row.get::<_, i64>(0))?;
            for id in found {
                ids.insert(id?);
            }
        }
        let mut still = c.prepare_cached("SELECT record FROM machine_log WHERE id = ?1")?;
        let mut rows = Vec::new();
        for id in ids.into_iter().rev().take(most) {
            let record = still.query_row(params![id], |row| row.get::<_, Vec<u8>>(0)).optional()?;
            rows.extend(record);
        }
        let mut notices = Vec::new();
        for bytes in rows.iter().rev() {
            let kept: Kept = shelf::decode(bytes)?;
            let Some((machine,by,action)) = notification(&kept.record.actor,&kept.record.action) else {continue};
            for (name,opened) in accounts {
                if kept.notified.get(name) == Some(opened) {
                    notices.push(fastctx_types::sharing::Notice {id:kept.record.id,at:kept.record.at,account:name.clone(),machine:machine.clone(),by:by.cloned(),action:action.clone()});
                }
            }
        }
        Ok(notices)
    }).map_err(shelf::other)
}
