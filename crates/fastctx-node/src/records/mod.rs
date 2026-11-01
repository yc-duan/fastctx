//! Bounded, indexed records shared by conversation-local state machines.
//! Families choose their own retention; deleting a conversation deletes every
//! family. Readers page by key, and a writer can change several families atomically.

#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
pub(crate) mod items;
pub(crate) mod legacy;
pub(crate) mod maps;
pub(crate) mod metadata;
mod migration;
pub(crate) use migration::{Source, ensure_each};

use fastctx_storage::{
    Database, Error, Priority, Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::layout::ConversationDir;
use serde::{Serialize, de::DeserializeOwned};
use std::io;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS context_projection_dirty(conversation TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS conversation_records (
    position INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation TEXT NOT NULL, family TEXT NOT NULL, identity TEXT NOT NULL,
    body BLOB NOT NULL, modified TEXT NOT NULL, expires INTEGER, arrival BLOB, category TEXT,
    UNIQUE(conversation,family,identity)
);
CREATE INDEX IF NOT EXISTS conversation_records_queue ON conversation_records(conversation,family,category,arrival,identity);
CREATE INDEX IF NOT EXISTS conversation_records_category ON conversation_records(conversation,family,category,identity);
CREATE INDEX IF NOT EXISTS conversation_records_order ON conversation_records(conversation,family,position);
CREATE INDEX IF NOT EXISTS conversation_records_arrival ON conversation_records(conversation,family,arrival,identity);
CREATE INDEX IF NOT EXISTS conversation_records_action ON conversation_records(conversation,json_extract(body,'$.hand'),json_extract(body,'$.result'),position) WHERE family='actions';
CREATE INDEX IF NOT EXISTS conversation_records_message ON conversation_records(conversation,json_extract(body,'$.kind'),json_extract(body,'$.turn')) WHERE family='messages';
CREATE INDEX IF NOT EXISTS conversation_records_expiry ON conversation_records(expires) WHERE expires IS NOT NULL;
CREATE TABLE IF NOT EXISTS conversation_record_links (
    conversation TEXT NOT NULL,family TEXT NOT NULL,member TEXT NOT NULL,identity TEXT NOT NULL,
    PRIMARY KEY(conversation,family,member,identity)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS conversation_record_versions (
    conversation TEXT NOT NULL, family TEXT NOT NULL, version INTEGER NOT NULL,
    PRIMARY KEY(conversation,family)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS conversation_record_imports (
    conversation TEXT NOT NULL, family TEXT NOT NULL,
    PRIMARY KEY(conversation,family)
) WITHOUT ROWID;
";

pub(crate) struct Rows<'a> {
    connection: &'a Connection,
    conversation: &'a str,
}

pub(crate) fn locate(directory: &ConversationDir) -> io::Result<(Database, String)> {
    crate::record_database::get(directory)
}

pub(crate) fn read<T>(directory: &ConversationDir, action: impl FnOnce(&Rows<'_>) -> Result<T>) -> io::Result<T> {
    read_using(directory, false, action)
}

pub(crate) fn read_background<T>(
    directory: &ConversationDir,
    action: impl FnOnce(&Rows<'_>) -> Result<T>,
) -> io::Result<T> {
    read_using(directory, true, action)
}

fn read_using<T>(
    directory: &ConversationDir,
    background: bool,
    action: impl FnOnce(&Rows<'_>) -> Result<T>,
) -> io::Result<T> {
    let (database, conversation) = locate(directory)?;
    let read = |connection: &Connection| {
        let transaction = connection.unchecked_transaction()?;
        let value = action(&Rows { connection: &transaction, conversation: &conversation })?;
        transaction.commit()?;
        Ok(value)
    };
    if background { database.read_background_now(read) } else { database.read_now(read) }.map_err(io::Error::other)
}

pub(crate) fn write_at<T: Send + 'static>(
    directory: &ConversationDir,
    priority: Priority,
    bytes: usize,
    action: impl FnOnce(&Rows<'_>) -> Result<T> + Send + 'static,
) -> io::Result<T> {
    write_then_at(directory, priority, bytes, action, |_| {})
}

pub(crate) fn write_then_at<T: Send + 'static>(
    directory: &ConversationDir,
    priority: Priority,
    bytes: usize,
    action: impl FnOnce(&Rows<'_>) -> Result<T> + Send + 'static,
    committed: impl FnOnce(&T) + Send + 'static,
) -> io::Result<T> {
    let (database, conversation) = locate(directory)?;
    if let Some(error) = database.fault() {
        return Err(io::Error::other(error.clone()));
    }
    if !directory.path().try_exists()? {
        fastctx_platform::ensure_private_dir(directory.path())?;
    }

    database
        .write_now_then_at(
            priority,
            bytes,
            move |connection| action(&Rows { connection, conversation: &conversation }),
            committed,
        )
        .map_err(io::Error::other)
}

impl Rows<'_> {
    pub(crate) fn connection(&self) -> &Connection {
        self.connection
    }
    pub(crate) fn conversation(&self) -> &str {
        self.conversation
    }

    pub(crate) fn get_raw(&self, family: &str, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .connection
            .query_row(
                "SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
                (self.conversation, family, key),
                |row| row.get(0),
            )
            .optional()?)
    }
    pub(crate) fn put_raw(&self, family: &str, key: &str, body: &[u8]) -> Result<()> {
        let category = std::path::Path::new(key).extension().and_then(|extension| extension.to_str()).unwrap_or("");
        let changed=self.connection.execute("INSERT INTO conversation_records(conversation,family,identity,body,modified,category) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(conversation,family,identity) DO UPDATE SET body=excluded.body,modified=excluded.modified,category=excluded.category WHERE conversation_records.body!=excluded.body",(self.conversation,family,key,body,jiff::Timestamp::now().to_string(),category))?;
        if changed != 0 {
            self.changed(family)?;
        }
        Ok(())
    }

    pub(crate) fn put_raw_ordered(&self, family: &str, key: &str, body: &[u8], order: &[u8]) -> Result<()> {
        self.put_raw(family, key, body)?;
        self.connection.execute("UPDATE conversation_records SET arrival=?4 WHERE conversation=?1 AND family=?2 AND identity=?3 AND arrival IS NOT ?4",(self.conversation,family,key,order))?;
        Ok(())
    }

    pub(crate) fn expire(&self, family: &str, key: &str, at: jiff::Timestamp) -> Result<()> {
        self.connection.execute(
            "UPDATE conversation_records SET expires=?4 WHERE conversation=?1 AND family=?2 AND identity=?3",
            (self.conversation, family, key, at.as_millisecond()),
        )?;
        Ok(())
    }

    pub(crate) fn get<T: DeserializeOwned>(&self, family: &str, key: &str) -> Result<Option<T>> {
        let body: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
                (self.conversation, family, key),
                |row| row.get(0),
            )
            .optional()?;
        body.as_deref().map(decode).transpose()
    }

    pub(crate) fn put<T: Serialize>(&self, family: &str, key: &str, value: &T) -> Result<()> {
        self.put_until(family, key, value, None)
    }

    pub(crate) fn put_until<T: Serialize>(
        &self,
        family: &str,
        key: &str,
        value: &T,
        expires: Option<jiff::Timestamp>,
    ) -> Result<()> {
        let bytes = encode(value)?;
        let changed=self.connection.execute("INSERT INTO conversation_records(conversation,family,identity,body,modified,expires) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(conversation,family,identity) DO UPDATE SET body=excluded.body,modified=excluded.modified,expires=excluded.expires
            WHERE conversation_records.body!=excluded.body OR conversation_records.expires IS NOT excluded.expires",
            params![self.conversation,family,key,bytes,jiff::Timestamp::now().to_string(),expires.map(|at|at.as_millisecond())])?;
        if changed != 0 {
            self.changed(family)?;
        }
        Ok(())
    }

    pub(crate) fn put_ordered<T: Serialize>(
        &self,
        family: &str,
        key: &str,
        value: &T,
        at: jiff::Timestamp,
    ) -> Result<()> {
        self.put(family, key, value)?;
        let order = (at.as_nanosecond() ^ i128::MIN).to_be_bytes();
        self.connection.execute("UPDATE conversation_records SET arrival=?4 WHERE conversation=?1 AND family=?2 AND identity=?3 AND arrival IS NOT ?4",(self.conversation,family,key,order.as_slice()))?;
        Ok(())
    }

    /// Replace an auxiliary receipt without changing the original intent time.
    pub(crate) fn put_retaining_time<T: Serialize>(&self, family: &str, key: &str, value: &T) -> Result<()> {
        let modified = self.modified(family, key)?;
        self.put(family, key, value)?;
        if let Some(modified) = modified {
            self.connection.execute("UPDATE conversation_records SET modified=?4 WHERE conversation=?1 AND family=?2 AND identity=?3 AND modified!=?4",(self.conversation,family,key,modified.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn delete(&self, family: &str, key: &str) -> Result<bool> {
        let changed = self.connection.execute(
            "DELETE FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
            (self.conversation, family, key),
        )? != 0;
        if changed {
            self.changed(family)?;
        }
        Ok(changed)
    }

    pub(crate) fn page<T: DeserializeOwned>(&self, family: &str, after: u64, limit: usize) -> Result<Vec<(u64, T)>> {
        let after = i64::try_from(after).map_err(|_| Error::new("record cursor exhausted"))?;
        let mut query=self.connection.prepare_cached("SELECT position,body FROM conversation_records WHERE conversation=?1 AND family=?2 AND position>?3 ORDER BY position LIMIT ?4")?;
        let rows = query.query_map((self.conversation, family, after, limit.clamp(1, 128) as i64), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        for row in rows {
            let (key, body) = row?;
            if !result.is_empty() && bytes + body.len() > 2 * 1024 * 1024 {
                break;
            }
            bytes += body.len();
            result.push((key as u64, decode(&body)?));
        }
        Ok(result)
    }

    pub(crate) fn modified(&self, family: &str, key: &str) -> Result<Option<jiff::Timestamp>> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT modified FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
                (self.conversation, family, key),
                |row| row.get(0),
            )
            .optional()?;
        value.map(|value| value.parse().map_err(|error: jiff::Error| Error::new(error.to_string()))).transpose()
    }

    pub(crate) fn link(&self, family: &str, member: &str, key: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR IGNORE INTO conversation_record_links(conversation,family,member,identity) VALUES(?1,?2,?3,?4)",
            (self.conversation, family, member, key),
        )?;
        Ok(())
    }

    pub(crate) fn linked<T: DeserializeOwned>(&self, family: &str, member: &str) -> Result<Vec<T>> {
        let mut query=self.connection.prepare_cached("SELECT r.body FROM conversation_record_links l JOIN conversation_records r ON r.conversation=l.conversation AND r.family=l.family AND r.identity=l.identity WHERE l.conversation=?1 AND l.family=?2 AND l.member=?3 ORDER BY l.identity")?;
        query
            .query_map((self.conversation, family, member), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| decode(&row?))
            .collect()
    }

    pub(crate) fn all<T: DeserializeOwned>(&self, family: &str) -> Result<Vec<T>> {
        let mut result = Vec::new();
        let mut after = 0;
        loop {
            let page = self.page(family, after, 128)?;
            let Some(last) = page.last() else { break };
            after = last.0;
            result.extend(page.into_iter().map(|(_, value)| value));
        }
        Ok(result)
    }

    pub(crate) fn unsettled<T: DeserializeOwned>(&self, family: &str, receipt: &str, other: &str) -> Result<Vec<T>> {
        let mut query=self.connection.prepare_cached("SELECT r.body FROM conversation_records r WHERE r.conversation=?1 AND r.family=?2
            AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.family=?3 AND d.identity=r.identity)
            AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.family=?4 AND d.identity=r.identity)
            ORDER BY r.arrival,r.identity")?;
        query
            .query_map((self.conversation, family, receipt, other), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| decode(&row?))
            .collect()
    }
    pub(crate) fn recent(&self, family: &str, since: jiff::Timestamp, through: jiff::Timestamp) -> Result<bool> {
        Ok(self.connection.query_row("SELECT 1 FROM conversation_records WHERE conversation=?1 AND family=?2 AND julianday(modified)>julianday(?3) AND julianday(modified)<=julianday(?4) LIMIT 1",(self.conversation,family,since.to_string(),through.to_string()),|row|row.get::<_,i32>(0)).optional()?.is_some())
    }

    pub(crate) fn key_page<T: DeserializeOwned>(
        &self,
        family: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<(String, T)>> {
        let mut query=self.connection.prepare_cached("SELECT identity,body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity>?3 ORDER BY identity LIMIT ?4")?;
        let mapped = query.query_map((self.conversation, family, after, limit.clamp(1, 128) as i64), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for row in mapped {
            let (key, body) = row?;
            if !result.is_empty() && bytes + body.len() > 2 * 1024 * 1024 {
                break;
            }
            bytes += body.len();
            result.push((key, decode(&body)?));
        }
        Ok(result)
    }

    pub(crate) fn event(&self, key: &str, event: &fastctx_types::session::SessionEvent) -> Result<bool> {
        let key = crate::speech::fixed_id("event", &[key]);
        crate::session_record::append_in(self.connection, self.conversation, key.as_str(), event)
    }

    fn changed(&self, family: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO conversation_record_versions(conversation,family,version) VALUES(?1,?2,1)
            ON CONFLICT(conversation,family) DO UPDATE SET version=version+1",
            (self.conversation, family),
        )?;
        Ok(())
    }
}

pub(crate) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| Error::new(error.to_string()))
}
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| Error::with_kind(fastctx_storage::ErrorKind::Corrupt, error.to_string()))
}
