//! Deeds open independently after verified import. A source cleanup proof lives
//! beyond ordinary deed retirement, so a crash cannot replay folded activity.
use super::legacy::{self, Archive, Context};
use crate::record_migration::scheduler;
use fastctx_storage::{
    Database,
    rusqlite::{Connection, OptionalExtension, types::Value},
};
use std::{io, sync::Arc};

pub(super) fn ready(database: &Database) -> io::Result<bool> {
    database.read_background_now(|db| present(db, "all", "")).map_err(io::Error::other)
}
fn present(db: &Connection, kind: &str, identity: &str) -> fastctx_storage::Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM book_imports WHERE kind=?1 AND identity=?2)",
        (kind, identity),
        |row| row.get(0),
    )?)
}
fn request_deed(context: &Context, key: &str) -> io::Result<()> {
    if context.target.read_background_now(|db| present(db, "deed", key)).map_err(io::Error::other)? {
        return Ok(());
    }
    let owned = context.clone();
    let identity = key.to_string();
    scheduler::request(&context.world, "@book", &format!("book-deed/{key}"), move || import_deed(&owned, &identity))
}
/// Every observer, including Git observations, waits at this same deed gate.
pub(super) fn deed(node: &crate::Node, key: &str) -> io::Result<()> {
    if ready(&node.book_database)? {
        return Ok(());
    }
    request_deed(&Context::of(node), key)
}
pub(super) fn ensure(node: &crate::Node) -> io::Result<()> {
    if ready(&node.book_database)? {
        return Ok(());
    }
    let context = Context::of(node);
    let world = context.world.clone();
    scheduler::request(&world, "@book", "book-all", move || import_all(&context))
}
fn proof(context: &Context, key: &str) -> io::Result<bool> {
    context
        .target
        .read_background_now(|db| {
            Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM book_migration_proofs WHERE identity=?1)", [key], |row| {
                row.get(0)
            })?)
        })
        .map_err(io::Error::other)
}
fn mark(context: &Context, key: &str, path: &std::path::Path, damage: bool) -> io::Result<()> {
    let (key, path) = (key.to_string(), path.to_string_lossy().into_owned());
    context.target.write_now_at(scheduler::priority(), key.len() + path.len(), move |db| {
        if !damage {
            let went: Option<String> = db.query_row("SELECT went FROM book_deeds WHERE deed=?1", [&key], |row| row.get(0)).optional()?;
            for machine in went.as_deref().unwrap_or_default().split_whitespace() { db.execute("INSERT OR IGNORE INTO book_visits(deed,machine) VALUES(?1,?2)", (&key, machine))?; }
            db.execute("INSERT OR IGNORE INTO book_complete(deed) SELECT deed FROM book_deeds WHERE deed=?1 AND ended IS NOT NULL", [&key])?;
        }
        db.execute("INSERT OR REPLACE INTO book_migration_proofs(identity,archive) VALUES(?1,?2)", (&key, path))?;
        db.execute("INSERT OR IGNORE INTO book_imports(kind,identity) VALUES('deed',?1)", [key])?;
        Ok(())
    }).map_err(io::Error::other)
}
fn cleanup_deed(context: &Context, key: &str) -> io::Result<()> {
    loop {
        scheduler::checkpoint()?;
        let rows = legacy::touch_page(context, key, None)?;
        if rows.is_empty() {
            break;
        }
        legacy::delete_rows(context, "book_touches", rows)?;
    }
    if legacy::exists(context, "book_deeds")? {
        let key = key.to_string();
        context
            .source
            .write_now_at(scheduler::priority(), key.len(), move |db| {
                db.execute("DELETE FROM book_deeds WHERE deed=?1", [key])?;
                Ok(())
            })
            .map_err(io::Error::other)?;
    }
    let key = key.to_string();
    context
        .target
        .write_now_at(scheduler::priority(), key.len(), move |db| {
            db.execute("DELETE FROM book_migration_proofs WHERE identity=?1", [key])?;
            Ok(())
        })
        .map_err(io::Error::other)
}
fn import_deed(context: &Context, key: &str) -> io::Result<()> {
    if proof(context, key)? {
        return cleanup_deed(context, key);
    }
    if context.target.read_background_now(|db| present(db, "deed", key)).map_err(io::Error::other)? {
        return Ok(());
    }
    let row = if legacy::exists(context, "book_deeds")? {
        context
            .source
            .read_background_now(|db| legacy::read(db, "book_deeds", &[Value::Text(key.into())]))
            .map_err(io::Error::other)?
    } else {
        None
    };
    let mut archive = Archive::open(context, key)?;
    let mut damage = None;
    if let Some(row) = &row {
        archive.append("book_deeds", std::slice::from_ref(row))?;
        if !legacy::valid("book_deeds", row) {
            damage = Some("invalid legacy deed fields".to_string());
        }
    }
    let mut after = None;
    loop {
        scheduler::checkpoint()?;
        let rows = legacy::touch_page(context, key, after.as_ref())?;
        let Some(last) = rows.last() else { break };
        after = Some(last[1].clone());
        archive.append("book_touches", &rows)?;
        if row.is_none() || rows.iter().any(|row| !legacy::valid("book_touches", row)) {
            damage = Some("invalid or orphaned legacy touches".to_string());
        }
    }
    if damage.is_none()
        && let Some(row) = &row
    {
        if let Err(error) = legacy::copy(context, "book_deeds", std::slice::from_ref(row)) {
            if error.kind() != io::ErrorKind::InvalidData {
                return Err(error);
            }
            damage = Some(error.to_string());
        }
        let mut after = None;
        while damage.is_none() {
            let rows = legacy::touch_page(context, key, after.as_ref())?;
            let Some(last) = rows.last() else { break };
            after = Some(last[1].clone());
            if let Err(error) = legacy::copy(context, "book_touches", &rows) {
                if error.kind() != io::ErrorKind::InvalidData {
                    return Err(error);
                }
                damage = Some(error.to_string());
            }
        }
    }
    let path = archive.finish(context, damage.as_deref())?;
    mark(context, key, &path, damage.is_some())?;
    cleanup_deed(context, key)
}
pub(super) fn import_all(context: &Context) -> io::Result<()> {
    let has_deeds = legacy::exists(context, "book_deeds")?;
    let has_touches = legacy::exists(context, "book_touches")?;
    for (table, present) in [("book_deeds", has_deeds), ("book_touches", has_touches)] {
        if !present {
            continue;
        }
        loop {
            let columns = legacy::columns(table).0;
            let rows = context
                .source
                .read_background_now(|db| {
                    Ok(db
                        .prepare(&format!("SELECT {columns} FROM {table} WHERE typeof(deed)<>'text' LIMIT 16"))?
                        .query_map([], |row| legacy::values(row, columns.split(',').count()))?
                        .collect::<std::result::Result<Vec<_>, _>>()?)
                })
                .map_err(io::Error::other)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let identity = serde_json::to_string(&legacy::encoded(std::slice::from_ref(&row)))?;
                let mut archive = Archive::open(context, &identity)?;
                archive.append(table, std::slice::from_ref(&row))?;
                archive.finish(context, Some("legacy deed identity is not text"))?;
                legacy::delete_rows(context, table, vec![row])?;
            }
        }
    }
    if has_deeds || has_touches {
        let query = match (has_deeds, has_touches) {
            (true, true) => "SELECT deed FROM book_deeds UNION SELECT deed FROM book_touches ORDER BY deed LIMIT 64",
            (true, false) => "SELECT deed FROM book_deeds ORDER BY deed LIMIT 64",
            _ => "SELECT DISTINCT deed FROM book_touches ORDER BY deed LIMIT 64",
        };
        loop {
            scheduler::checkpoint()?;
            let keys = context
                .source
                .read_background_now(|db| {
                    Ok(db
                        .prepare_cached(query)?
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?)
                })
                .map_err(io::Error::other)?;
            if keys.is_empty() {
                break;
            }
            for key in keys {
                if proof(context, &key)? {
                    cleanup_deed(context, &key)?;
                } else {
                    request_deed(context, &key)?;
                }
            }
        }
    }
    if legacy::exists(context, "book_hour")? {
        loop {
            scheduler::checkpoint()?;
            let rows = context
                .source
                .read_background_now(|db| {
                    Ok(db
                        .prepare_cached(&format!(
                            "SELECT {} FROM book_hour ORDER BY at,session,agent,held,account LIMIT 16",
                            legacy::HOURS
                        ))?
                        .query_map([], |row| legacy::values(row, 9))?
                        .collect::<std::result::Result<Vec<_>, _>>()?)
                })
                .map_err(io::Error::other)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let identity = serde_json::to_string(&legacy::encoded(&[row[..5].to_vec()]))?;
                let mut archive = Archive::open(context, &format!("hour/{identity}"))?;
                archive.append("book_hour", std::slice::from_ref(&row))?;
                let damage = if !legacy::valid("book_hour", &row) {
                    Some("invalid legacy hour fields".to_string())
                } else {
                    match legacy::copy(context, "book_hour", std::slice::from_ref(&row)) {
                        Ok(()) => None,
                        Err(error) if error.kind() == io::ErrorKind::InvalidData => Some(error.to_string()),
                        Err(error) => return Err(error),
                    }
                };
                archive.finish(context, damage.as_deref())?;
                // No observer changes hours until the final all-imported commit.
                // If interrupted here, immutable row comparison permits replay.
                legacy::delete_rows(context, "book_hour", vec![row])?;
            }
        }
    }
    context
        .target
        .write_now_at(scheduler::priority(), 32, |db| {
            db.execute("INSERT OR IGNORE INTO book_imports(kind,identity) VALUES('all','')", [])?;
            Ok(())
        })
        .map_err(io::Error::other)
}
pub(crate) fn start(node: &Arc<crate::Node>) {
    if node.book_database.fault().is_some() {
        return;
    }
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let working = node.clone();
            match fastctx_platform::task::spawn_blocking(move || scheduler::discovery(|| ensure(&working))).await {
                Ok(Ok(())) => return,
                Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                Ok(Err(error)) => node.log.line(format_args!("migrating the activity book: {error}")),
                Err(error) => node.log.line(format_args!("activity book migration worker: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}
