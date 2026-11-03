//! A fold first freezes one deed, accounts for it once, then removes detail in
//! small transactions. Restarting resumes the durable fold instead of recounting.

use super::*;
use fastctx_storage::{Priority, rusqlite::OptionalExtension};
use std::io;

pub(crate) fn fold(node: &Node) -> io::Result<()> {
    if !super::migrate::ready(&node.book_database)? {
        return Ok(());
    }
    let now = jiff::Timestamp::now();
    let deeds_before = now.checked_sub(DEEDS_KEPT).map_err(io::Error::other)?.as_millisecond();
    let hours_before = now.checked_sub(HOURS_KEPT).map_err(io::Error::other)?.as_millisecond();
    loop {
        let key = node
            .book_database
            .write_now_at(Priority::Background, 128, move |db| {
                let pending: Option<String> = db
                    .query_row("SELECT deed FROM book_folding ORDER BY deed LIMIT 1", [], |row| row.get(0))
                    .optional()?;
                if pending.is_some() {
                    return Ok(pending);
                }
                let next: Option<String> = db
                    .query_row(
                        "SELECT deed FROM book_deeds WHERE ended<?1 AND EXISTS (SELECT 1 FROM book_complete c WHERE c.deed=book_deeds.deed) ORDER BY began LIMIT 1",
                        [deeds_before],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(key) = &next {
                    db.execute("INSERT INTO book_folding(deed,applied) VALUES(?1,0)", [key])?;
                }
                Ok(next)
            })
            .map_err(io::Error::other)?;
        let Some(key) = key else { break };
        let (accounting, files) = node
            .book_database
            .read_now(|db| {
                let applied: bool =
                    db.query_row("SELECT applied FROM book_folding WHERE deed=?1", [&key], |row| row.get(0))?;
                let files: i64 = if applied {
                    0
                } else {
                    db.query_row("SELECT count(*) FROM book_touches WHERE deed=?1", [&key], |row| row.get(0))?
                };
                Ok((!applied, files))
            })
            .map_err(io::Error::other)?;
        if accounting {
            let key = key.clone();
            node.book_database.write_now_at(Priority::Background,key.len(),move |db| {
                db.execute("INSERT INTO book_hour(at,session,agent,held,account,runs,failed,busy,files)
                    SELECT (began/3600000)*3600000,coalesce(session,''),coalesce(agent,''),held,account,1,
                        CASE WHEN outcome IN ('failed','lost') THEN 1 ELSE 0 END,max(coalesce(ended,began)-began,0),?2
                    FROM book_deeds WHERE deed=?1
                    ON CONFLICT(at,session,agent,held,account) DO UPDATE SET runs=runs+excluded.runs,failed=failed+excluded.failed,busy=busy+excluded.busy,files=files+excluded.files",params![key,files])?;
                db.execute("UPDATE book_folding SET applied=1 WHERE deed=?1",[key])?;
                Ok(())
            }).map_err(io::Error::other)?;
        }
        for (table, column) in [
            ("book_touches", "path"),
            ("book_steps", "identity"),
            ("book_writes", "identity"),
            ("book_visits", "machine"),
        ] {
            loop {
                let key = key.clone();
                let changed=node.book_database.write_now_at(Priority::Background,128,move |db| {
                    Ok(db.execute(&format!("DELETE FROM {table} WHERE deed=?1 AND {column} IN (SELECT {column} FROM {table} WHERE deed=?1 ORDER BY {column} LIMIT 64)"),[key])?)
                }).map_err(io::Error::other)?;
                if changed == 0 {
                    break;
                }
            }
        }
        node.book_database
            .write_now_at(Priority::Background, key.len(), move |db| {
                db.execute("DELETE FROM book_cursors WHERE deed=?1", [&key])?;
                db.execute("DELETE FROM book_complete WHERE deed=?1", [&key])?;
                db.execute("DELETE FROM book_deeds WHERE deed=?1", [&key])?;
                db.execute("DELETE FROM book_imports WHERE kind='deed' AND identity=?1", [&key])?;
                db.execute("INSERT OR IGNORE INTO book_retired(deed,at) VALUES(?1,?2)", (&key, now.as_millisecond()))?;
                db.execute("DELETE FROM book_folding WHERE deed=?1", [key])?;
                Ok(())
            })
            .map_err(io::Error::other)?;
    }
    loop {
        let changed = node
            .book_database
            .write_now_at(Priority::Background, 128, move |db| {
                Ok(db.execute(
                    "DELETE FROM book_hour WHERE (at,session,agent,held,account) IN
                (SELECT at,session,agent,held,account FROM book_hour WHERE at<?1 ORDER BY at LIMIT 64)",
                    [hours_before],
                )? + db.execute(
                    "DELETE FROM book_retired WHERE deed IN (SELECT deed FROM book_retired WHERE at<?1 LIMIT 64)",
                    [hours_before],
                )?)
            })
            .map_err(io::Error::other)?;
        if changed == 0 {
            break;
        }
    }
    Ok(())
}
