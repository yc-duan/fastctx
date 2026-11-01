//! A rejected offer is quarantined and only rows inserted by that offer are
//! undone. The node's existing rows and unrelated conversations remain usable.
use super::{cursors, schema, triggers};
use fastctx_platform::readings;
use fastctx_storage::{
    Database, Error, Priority, Result,
    rusqlite::{OptionalExtension, params_from_iter, types::Value},
};
use std::{io, path::Path};

pub(super) fn resume(world: &Path, database: &Database, conversation: &str, reason: String) -> io::Result<()> {
    let (scope, kept_reason) = (conversation.to_string(), reason.clone());
    let recorded = database
        .write_now_at(Priority::Control, reason.len() + 128, move |db| {
            Ok(db.execute(
                "UPDATE node_conversation_transfers SET failure=coalesce(failure,?2) WHERE name=?1",
                (&scope, kept_reason),
            )? != 0)
        })
        .map_err(io::Error::other)?;
    if !recorded {
        super::cancelled(world, conversation);
        return Ok(());
    }
    super::quarantine::keep(world, database, conversation, &reason)?;
    rollback(database, conversation).map_err(io::Error::other)?;
    let scope = conversation.to_string();
    let finished = database
        .write_now_at(Priority::Control, 256, move |db| {
            db.execute("DELETE FROM node_conversation_transfer_progress WHERE name=?1", [&scope])?;
            Ok(db.execute("UPDATE node_conversation_transfers SET rejected=1 WHERE name=?1 AND rejected=0", [scope])?
                != 0)
        })
        .map_err(io::Error::other)?;
    if finished {
        readings::counter("migration.imports_rejected").add(1);
        super::record_rejection(world, conversation, reason);
    } else {
        let reason = database
            .read_background_now(|db| {
                Ok(db
                    .query_row(
                        "SELECT failure FROM node_conversation_transfers WHERE name=?1 AND rejected=1",
                        [conversation],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .map_err(io::Error::other)?;
        if let Some(reason) = reason {
            super::record_rejection(world, conversation, reason);
        } else {
            super::cancelled(world, conversation);
        }
    }
    Ok(())
}

fn rollback(database: &Database, conversation: &str) -> Result<()> {
    let tables = database.read_background_now(|db| schema::children_first(db, schema::tables(db)?))?;
    for table in tables {
        loop {
            let keys=database.read_background_now(|db| {
                Ok(db.prepare("SELECT row_key FROM node_conversation_import_rows WHERE name=?1 AND table_name=?2 ORDER BY row_key LIMIT 128")?
                    .query_map((conversation,&table.name),|row|row.get::<_,Vec<u8>>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
            })?;
            if keys.is_empty() {
                break;
            }
            let bytes = keys.iter().map(Vec::len).sum::<usize>() + 256;
            let (scope, table) = (conversation.to_string(), table.clone());
            let removed = database.write_now_at(Priority::Background, bytes, move |db| {
                let active: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM node_conversation_transfers WHERE name=?1 AND rejected=0)",
                    [&scope],
                    |row| row.get(0),
                )?;
                if !active {
                    return Ok(None);
                }
                let _triggers = triggers::SuspendedTriggers::new(db)?;
                let predicate = table
                    .primary
                    .iter()
                    .map(|column| format!("{} IS ?", schema::quote(column)))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let sql = format!("DELETE FROM {} WHERE conversation=? AND {predicate}", schema::quote(&table.name));
                let mut removed = 0;
                for encoded in keys {
                    let mut key = cursors::decode(&encoded)?;
                    if key.len() != table.primary.len() {
                        return Err(Error::new("conversation import rollback key changed shape"));
                    }
                    key.insert(0, Value::Text(scope.clone()));
                    removed += db.prepare_cached(&sql)?.execute(params_from_iter(key))?;
                    db.execute(
                        "DELETE FROM node_conversation_import_rows WHERE name=?1 AND table_name=?2 AND row_key=?3",
                        (&scope, &table.name, encoded),
                    )?;
                }
                Ok(Some(removed))
            })?;
            let Some(removed) = removed else {
                return Ok(());
            };
            readings::counter("migration.import_rows_rolled_back").add(removed as u64);
        }
    }
    let remaining: bool = database.read_background_now(|db| {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM node_conversation_import_rows WHERE name=?1)",
            [conversation],
            |row| row.get(0),
        )?)
    })?;
    if remaining {
        return Err(Error::new("conversation import rollback refers to an unknown table"));
    }
    Ok(())
}

pub(crate) fn discard_journal(database: &Database, conversation: &str) -> Result<()> {
    loop {
        let scope = conversation.to_string();
        let removed=database.write_now_at(Priority::Background,128*1024,move |db| {
            Ok(db.execute("DELETE FROM node_conversation_import_rows WHERE (name,table_name,row_key) IN (SELECT name,table_name,row_key FROM node_conversation_import_rows WHERE name=?1 LIMIT 128)",[scope])?)
        })?;
        if removed == 0 {
            return Ok(());
        }
    }
}
