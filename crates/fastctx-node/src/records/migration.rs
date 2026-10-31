//! Record families stream into restartable batches, verified before cutover.

use super::*;
use std::{path::PathBuf, sync::LazyLock};

static GATES: LazyLock<crate::record_gates::Gates> = LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "migrating record family")
});

pub(crate) struct Source {
    pub key: String,
    pub path: PathBuf,
    pub body: Vec<u8>,
    pub modified: jiff::Timestamp,
    pub arrival: Option<Vec<u8>>,
    pub replace: bool,
    pub category: Option<String>,
    pub expires: Option<jiff::Timestamp>,
}
impl Source {
    pub(crate) fn expiring(mut self, at: jiff::Timestamp) -> Self {
        self.expires = Some(at);
        self
    }

    pub(crate) fn ordered_bytes(mut self, order: Vec<u8>) -> Self {
        self.arrival = Some(order);
        self
    }

    pub(crate) fn categorized(mut self, category: String) -> Self {
        self.category = Some(category);
        self
    }

    pub(crate) fn replacing(mut self) -> Self {
        self.replace = true;
        self
    }

    pub(crate) fn ordered(mut self, at: jiff::Timestamp) -> Self {
        self.arrival = Some((at.as_nanosecond() ^ i128::MIN).to_be_bytes().to_vec());
        self
    }

    pub(crate) fn new(key: String, path: PathBuf, body: Vec<u8>) -> io::Result<Self> {
        let modified = jiff::Timestamp::try_from(std::fs::metadata(&path)?.modified()?).map_err(io::Error::other)?;
        Ok(Self { key, path, body, modified, arrival: None, replace: false, category: None, expires: None })
    }
}

pub(crate) fn ensure_each(
    directory: &ConversationDir,
    family: &str,
    source: impl FnOnce(&mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
) -> io::Result<()> {
    let (database, conversation) = locate(directory)?;
    let imported = |connection: &Connection| -> Result<bool> {
        Ok(connection
            .query_row(
                "SELECT 1 FROM conversation_record_imports WHERE conversation=?1 AND family=?2",
                (&conversation, family),
                |row| row.get::<_, i32>(0),
            )
            .optional()?
            .is_some())
    };
    if database.read_now(imported).map_err(io::Error::other)? {
        return Ok(());
    }
    let gate = GATES.get(&directory.path().join(format!("record-family-{family}")));
    let _gate = gate.lock();
    if database.read_now(imported).map_err(io::Error::other)? {
        return Ok(());
    }
    let mut pending = Vec::new();
    let mut bytes = 0;
    source(&mut |row| {
        bytes += row.body.len();
        pending.push(row);
        if pending.len() >= 64 || bytes >= 256 * 1024 {
            commit(&database, &conversation, family, std::mem::take(&mut pending))?;
            bytes = 0;
        }
        Ok(())
    })?;
    commit(&database, &conversation, family, pending)?;
    let family = family.to_string();
    database
        .write_now_at(Priority::Background, 128, move |connection| {
            connection.execute(
                "INSERT OR IGNORE INTO conversation_record_imports(conversation,family) VALUES(?1,?2)",
                (conversation, family),
            )?;
            Ok(())
        })
        .map_err(io::Error::other)
}

fn commit(database: &Database, conversation: &str, family: &str, sources: Vec<Source>) -> io::Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let scope = conversation.to_string();
    let family = family.to_string();
    let rows = sources
        .iter()
        .map(|source| {
            (
                source.key.clone(),
                source.body.clone(),
                source.modified.to_string(),
                source.arrival.clone(),
                source.replace,
                source.category.clone(),
                source.expires.map(|at| at.as_millisecond()),
            )
        })
        .collect::<Vec<_>>();
    let bytes = rows.iter().map(|(key, body, _, _, _, _, _)| key.len() + body.len() + 128).sum();
    let group = family.clone();
    database.write_now_at(Priority::Background,bytes,move |connection| {
        for (key,body,modified,arrival,replace,category,expires) in rows {
            connection.execute("INSERT INTO conversation_records(conversation,family,identity,body,modified,arrival,category,expires) VALUES(?1,?2,?3,?4,?5,?6,?8,?9) ON CONFLICT(conversation,family,identity) DO UPDATE SET body=excluded.body,modified=excluded.modified,arrival=excluded.arrival,category=excluded.category,expires=excluded.expires WHERE ?7 AND (conversation_records.body!=excluded.body OR conversation_records.modified!=excluded.modified OR conversation_records.arrival IS NOT excluded.arrival OR conversation_records.category IS NOT excluded.category OR conversation_records.expires IS NOT excluded.expires)",params![scope,group,key,body,modified,arrival,replace,category,expires])?;
        }
        Ok(())
    }).map_err(io::Error::other)?;
    database
        .read_now(|connection| {
            let mut checked = std::collections::HashSet::new();
            for source in sources.iter().rev() {
                if !checked.insert(&source.key) {
                    continue;
                }
                let (body,modified,arrival,category,expires) = connection.query_row(
                    "SELECT body,modified,arrival,category,expires FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
                    (conversation, &family, &source.key),
                    |row| Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<Vec<u8>>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<i64>>(4)?)),
                )?;
                if body != source.body || modified!=source.modified.to_string() || arrival!=source.arrival || category!=source.category || expires!=source.expires.map(|at|at.as_millisecond()) {
                    return Err(Error::new(format!("migrating {} did not verify", source.path.display())));
                }
            }
            Ok(())
        })
        .map_err(io::Error::other)
}
