//! Machine-log indexes preserve the readers fixed when a change was made.
//! Account incarnation and device key are part of every visibility lookup.

use super::shelf;
use crate::machine_log::{Kept, RETENTION, notification};
use fastctx_storage::{
    Database, Error,
    rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value},
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
    }
}
fn mask(reads: Reads) -> i64 {
    [Needs::Renames, Needs::Forgets, Needs::Versions, Needs::Due, Needs::ProjectDeletes, Needs::Profiles]
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

/// Removes one bounded batch, together with its visibility indexes.
pub(crate) fn expire(connection: &Connection, cutoff: Timestamp) -> fastctx_storage::Result<usize> {
    Ok(connection.prepare_cached("DELETE FROM machine_log WHERE id IN (SELECT id FROM machine_log WHERE (at,nano) <= (?1,?2) ORDER BY at,nano LIMIT 512)")?
        .execute(params![cutoff.as_second(), cutoff.subsec_nanosecond()])?)
}

pub(crate) fn expires(database: &Database) -> std::io::Result<Option<Timestamp>> {
    database
        .read_now(|c| {
            let first: Option<(i64, i32)> = c
                .prepare_cached("SELECT at,nano FROM machine_log ORDER BY at,nano LIMIT 1")?
                .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            first
                .map(|(sec, nano)| {
                    Timestamp::new(sec, nano)
                        .and_then(|at| at.checked_add(RETENTION))
                        .map_err(|e| Error::new(e.to_string()))
                })
                .transpose()
        })
        .map_err(shelf::other)
}

pub(crate) fn updated(
    database: &Database,
    key: &fastctx_net::sign::PublicKey,
    was: &str,
    now: &str,
) -> std::io::Result<bool> {
    let cutoff = Timestamp::now().checked_sub(RETENTION).map_err(std::io::Error::other)?;
    database
        .read_now(|c| {
            Ok(c.prepare_cached("SELECT 1 FROM machine_log_updated u JOIN machine_log l ON l.id=u.id WHERE machine=?1 AND was=?2 AND now=?3 AND (l.at,l.nano)>(?4,?5) LIMIT 1")?
                .query_row(params![key.to_string(), was, now, cutoff.as_second(), cutoff.subsec_nanosecond()], |_| Ok(()))
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
    now: Timestamp,
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
        let cutoff = now.checked_sub(RETENTION).map_err(|e|Error::new(e.to_string()))?;
        loop {
            let mut records = std::collections::BTreeMap::new();
            let mut more = false;
            for reader in readers {
                let mut stmt = c.prepare_cached("SELECT r.id,l.record FROM machine_log_readers r JOIN machine_log l ON l.id=r.id WHERE reader=?1 AND r.id<=?2 AND (l.needs & ?3)=l.needs AND (l.at,l.nano)>(?4,?5) ORDER BY r.id DESC LIMIT 256")?;
                let mut count = 0;
                for row in stmt.query_map(params![reader,cursor,mask(reads),cutoff.as_second(),cutoff.subsec_nanosecond()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?)))? {
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
    now: Timestamp,
) -> std::io::Result<Vec<fastctx_types::sharing::Notice>> {
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    database.read_now(|c| {
        let placeholders = vec!["?";accounts.len()].join(",");
        let sql = format!("SELECT DISTINCT l.id,l.record FROM machine_log_notified n JOIN machine_log l ON l.id=n.id WHERE n.reader IN ({placeholders}) AND (l.at,l.nano)>(?,?) ORDER BY l.id");
        let cutoff = now.checked_sub(RETENTION).map_err(|e|Error::new(e.to_string()))?;
        let mut values: Vec<Value> = accounts.iter().map(|(name,opened)|Value::Text(account(name,*opened))).collect();
        values.extend([Value::Integer(cutoff.as_second()),Value::Integer(i64::from(cutoff.subsec_nanosecond()))]);
        let mut stmt = c.prepare_cached(&sql)?;
        let mut cursor = stmt.query(params_from_iter(values))?;
        let mut notices = Vec::new();
        while let Some(row) = cursor.next()? {
            let kept: Kept = shelf::decode(&row.get::<_,Vec<u8>>(1)?)?;
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
