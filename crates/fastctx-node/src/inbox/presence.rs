//! Pending-input membership has a revision, independent of its mutable body.
//! A goal pause freezes one revision in O(1). Closed intervals survive only if
//! that frozen set still needs them; maintenance visits at most 64 per batch.
use crate::records::{self, Rows};
use fastctx_storage::{Database, Priority, Result, rusqlite::OptionalExtension};
use fastctx_types::{NoticeId, layout::ConversationDir};
use std::io;

pub(crate) const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS input_membership_heads(conversation TEXT PRIMARY KEY,revision INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS input_memberships(conversation TEXT NOT NULL,identity TEXT NOT NULL,opened INTEGER NOT NULL,closed INTEGER,PRIMARY KEY(conversation,identity,opened)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS input_memberships_closed ON input_memberships(conversation,identity,opened) WHERE closed IS NOT NULL;
CREATE TABLE IF NOT EXISTS input_freezes(conversation TEXT PRIMARY KEY,revision INTEGER NOT NULL) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS input_membership_insert AFTER INSERT ON conversation_records WHEN NEW.family='pending-inputs' AND NOT EXISTS(SELECT 1 FROM conversation_records WHERE conversation=NEW.conversation AND identity=NEW.identity AND family IN ('delivery-receipts','uncertain-inputs')) BEGIN
 INSERT INTO input_membership_heads(conversation,revision) VALUES(NEW.conversation,1) ON CONFLICT(conversation) DO UPDATE SET revision=revision+1;
 INSERT INTO input_memberships(conversation,identity,opened,closed) SELECT NEW.conversation,NEW.identity,revision,NULL FROM input_membership_heads WHERE conversation=NEW.conversation;
END;
CREATE TRIGGER IF NOT EXISTS input_membership_delete AFTER DELETE ON conversation_records WHEN OLD.family='pending-inputs' AND EXISTS(SELECT 1 FROM input_memberships WHERE conversation=OLD.conversation AND identity=OLD.identity AND closed IS NULL) BEGIN
 INSERT INTO input_membership_heads(conversation,revision) VALUES(OLD.conversation,1) ON CONFLICT(conversation) DO UPDATE SET revision=revision+1;
 UPDATE input_memberships SET closed=(SELECT revision FROM input_membership_heads WHERE conversation=OLD.conversation) WHERE conversation=OLD.conversation AND identity=OLD.identity AND closed IS NULL;
END;
CREATE TRIGGER IF NOT EXISTS input_membership_settled AFTER INSERT ON conversation_records WHEN NEW.family IN ('delivery-receipts','uncertain-inputs') AND EXISTS(SELECT 1 FROM input_memberships WHERE conversation=NEW.conversation AND identity=NEW.identity AND closed IS NULL) BEGIN
 INSERT INTO input_membership_heads(conversation,revision) VALUES(NEW.conversation,1) ON CONFLICT(conversation) DO UPDATE SET revision=revision+1;
 UPDATE input_memberships SET closed=(SELECT revision FROM input_membership_heads WHERE conversation=NEW.conversation) WHERE conversation=NEW.conversation AND identity=NEW.identity AND closed IS NULL;
END;
CREATE TRIGGER IF NOT EXISTS input_membership_reopened AFTER DELETE ON conversation_records WHEN OLD.family IN ('delivery-receipts','uncertain-inputs') AND EXISTS(SELECT 1 FROM conversation_records WHERE conversation=OLD.conversation AND identity=OLD.identity AND family='pending-inputs') AND NOT EXISTS(SELECT 1 FROM conversation_records WHERE conversation=OLD.conversation AND identity=OLD.identity AND family IN ('delivery-receipts','uncertain-inputs')) BEGIN
 INSERT INTO input_membership_heads(conversation,revision) VALUES(OLD.conversation,1) ON CONFLICT(conversation) DO UPDATE SET revision=revision+1;
 INSERT INTO input_memberships(conversation,identity,opened,closed) SELECT OLD.conversation,OLD.identity,revision,NULL FROM input_membership_heads WHERE conversation=OLD.conversation;
END;
";

/// Backfill earlier SQL stores in bounded batches before their first pause.
/// Concurrent insertions/deletions already run the triggers; only old live
/// rows without a membership interval receive the initial revision zero.
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    super::storage::initialize(directory)?;
    let (database, conversation) = records::locate(directory)?;
    let ready = database
        .read_now(|db| {
            Ok(db
                .query_row(
                    "SELECT 1 FROM conversation_record_imports WHERE conversation=?1 AND family='input-membership'",
                    [&conversation],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some())
        })
        .map_err(io::Error::other)?;
    if ready {
        return Ok(());
    }
    let through = database
        .read_now(|db| {
            Ok(db
                .query_row(
                    "SELECT max(position) FROM conversation_records WHERE conversation=?1 AND family='pending-inputs'",
                    [&conversation],
                    |row| row.get::<_, Option<i64>>(0),
                )?
                .unwrap_or(0))
        })
        .map_err(io::Error::other)?;
    let mut after = 0;
    loop {
        let keys=database.read_now(|db| {
            let mut query=db.prepare_cached("SELECT position FROM conversation_records WHERE conversation=?1 AND family='pending-inputs' AND position>?2 AND position<=?3 ORDER BY position LIMIT 64")?;
            Ok(query.query_map((&conversation,after,through),|row|row.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).map_err(io::Error::other)?;
        let Some(last) = keys.last() else { break };
        after = *last;
        let scope = conversation.clone();
        database.write_now_at(Priority::Background,1024,move |db| {
            for position in keys {
                db.execute("INSERT OR IGNORE INTO input_memberships(conversation,identity,opened,closed) SELECT conversation,identity,0,NULL FROM conversation_records r WHERE conversation=?1 AND position=?2 AND family='pending-inputs' AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.identity=r.identity AND d.family IN ('delivery-receipts','uncertain-inputs')) AND NOT EXISTS(SELECT 1 FROM input_memberships i WHERE i.conversation=r.conversation AND i.identity=r.identity AND i.closed IS NULL)",(&scope,position))?;
            }
            Ok(())
        }).map_err(io::Error::other)?;
    }
    database
        .write_now_at(Priority::Background, 128, move |db| {
            db.execute(
                "INSERT OR IGNORE INTO conversation_record_imports(conversation,family) VALUES(?1,'input-membership')",
                [conversation],
            )?;
            Ok(())
        })
        .map_err(io::Error::other)
}
pub(crate) fn revision(rows: &Rows<'_>) -> Result<u64> {
    Ok(rows
        .connection()
        .query_row("SELECT revision FROM input_membership_heads WHERE conversation=?1", [rows.conversation()], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(0) as u64)
}
pub(crate) fn freeze(rows: &Rows<'_>, revision: Option<u64>) -> Result<()> {
    if let Some(revision) = revision {
        rows.connection().execute("INSERT INTO input_freezes(conversation,revision) VALUES(?1,?2) ON CONFLICT(conversation) DO UPDATE SET revision=excluded.revision",(rows.conversation(),revision as i64))?;
    } else {
        rows.connection().execute("DELETE FROM input_freezes WHERE conversation=?1", [rows.conversation()])?;
    }
    Ok(())
}
pub(crate) fn held(rows: &Rows<'_>, revision: u64) -> Result<Vec<NoticeId>> {
    let mut query=rows.connection().prepare_cached("SELECT identity FROM input_memberships WHERE conversation=?1 AND opened<=?2 AND (closed IS NULL OR closed>?2) ORDER BY identity")?;
    query
        .query_map((rows.conversation(), revision as i64), |row| row.get::<_, String>(0))?
        .map(|row| {
            row?.parse().map_err(|error| fastctx_storage::Error::new(format!("invalid input identity: {error}")))
        })
        .collect()
}
pub(crate) fn collect(database: &Database) -> io::Result<()> {
    let mut cursor = (String::new(), String::new(), -1i64);
    loop {
        let rows=database.read_now(|db| {
            let mut query=db.prepare_cached("SELECT conversation,identity,opened FROM input_memberships WHERE closed IS NOT NULL AND (conversation,identity,opened)>(?1,?2,?3) ORDER BY conversation,identity,opened LIMIT 64")?;
            Ok(query.query_map((&cursor.0,&cursor.1,cursor.2),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).map_err(io::Error::other)?;
        let Some(last) = rows.last() else { return Ok(()) };
        cursor = last.clone();
        database.write_now_at(Priority::Background,4096,move |db| {
            for (scope,id,opened) in rows {
                db.execute("DELETE FROM input_memberships WHERE conversation=?1 AND identity=?2 AND opened=?3 AND closed IS NOT NULL AND NOT EXISTS(SELECT 1 FROM input_freezes f WHERE f.conversation=input_memberships.conversation AND input_memberships.opened<=f.revision AND input_memberships.closed>f.revision)",(scope,id,opened))?;
            }
            Ok(())
        }).map_err(io::Error::other)?;
    }
}
