//! Old book rows stay immutable in node.sqlite3 until their verified copy is
//! complete. A live deed imports first, so later observations cannot be overwritten.

use crate::Node;
use fastctx_storage::{
    Database, Priority, Result,
    rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value},
};
use std::{
    io,
    sync::{Arc, LazyLock},
};

static GATES: LazyLock<crate::record_gates::Gates> =
    LazyLock::new(|| crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "migrating book deed"));
const DEED_COLUMNS: &str = "deed,job,began,ended,from_machine,session,agent,held,project,account,script,outcome,code,output,went,looked,copied";
const TOUCH_COLUMNS: &str = "deed,path,how,added,removed,seen";
const HOUR_COLUMNS: &str = "at,session,agent,held,account,runs,failed,busy,files";

pub(super) fn ready(database: &Database) -> io::Result<bool> {
    database
        .read_now(|db| {
            Ok(db
                .query_row("SELECT 1 FROM book_imports WHERE kind='all' AND identity=''", [], |row| {
                    row.get::<_, i32>(0)
                })
                .optional()?
                .is_some())
        })
        .map_err(io::Error::other)
}

fn present(db: &Connection, kind: &str, identity: &str) -> Result<bool> {
    Ok(db
        .query_row("SELECT 1 FROM book_imports WHERE kind=?1 AND identity=?2", (kind, identity), |row| {
            row.get::<_, i32>(0)
        })
        .optional()?
        .is_some())
}

fn values(row: &fastctx_storage::rusqlite::Row<'_>, count: usize) -> fastctx_storage::rusqlite::Result<Vec<Value>> {
    (0..count).map(|i| row.get(i)).collect()
}

fn legacy(node: &Node, table: &str) -> io::Result<bool> {
    node.database
        .read_now(|db| {
            Ok(db
                .query_row("SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1", [table], |row| {
                    row.get::<_, i32>(0)
                })
                .optional()?
                .is_some())
        })
        .map_err(io::Error::other)
}

/// Must precede any new observation of this deed, on a blocking worker.
pub(super) fn deed(node: &Node, key: &str) -> io::Result<()> {
    if ready(&node.book_database)? {
        return Ok(());
    }
    let gate = GATES.get(&node.world.path().join("book-import").join(crate::speech::fixed_id("deed", &[key]).as_str()));
    let _importing = gate.lock();
    if node.book_database.read_now(|db| present(db, "deed", key)).map_err(io::Error::other)? {
        return Ok(());
    }
    if legacy(node, "book_deeds")? {
        let row = node
            .database
            .read_now(|db| {
                Ok(db
                    .query_row(&format!("SELECT {DEED_COLUMNS} FROM book_deeds WHERE deed=?1"), [key], |row| {
                        values(row, 17)
                    })
                    .optional()?)
            })
            .map_err(io::Error::other)?;
        if let Some(row) = row {
            copy(&node.book_database, "book_deeds", DEED_COLUMNS, vec![row])?;
            let mut after = String::new();
            loop {
                let rows = node
                    .database
                    .read_now(|db| {
                        let mut query = db.prepare(&format!(
                            "SELECT {TOUCH_COLUMNS} FROM book_touches WHERE deed=?1 AND path>?2 ORDER BY path LIMIT 16"
                        ))?;
                        Ok(query
                            .query_map((key, &after), |row| values(row, 6))?
                            .collect::<std::result::Result<Vec<_>, _>>()?)
                    })
                    .map_err(io::Error::other)?;
                let Some(last) = rows.last() else { break };
                after = match &last[1] {
                    Value::Text(path) => path.clone(),
                    _ => return Err(io::Error::other("invalid book path")),
                };
                copy(&node.book_database, "book_touches", TOUCH_COLUMNS, rows)?;
            }
        }
    }
    let key = key.to_string();
    node.book_database
        .write_now_at(Priority::Background, key.len(), move |db| {
            let went: Option<String> =
                db.query_row("SELECT went FROM book_deeds WHERE deed=?1", [&key], |row| row.get(0)).optional()?;
            for machine in went.as_deref().unwrap_or_default().split_whitespace() {
                db.execute("INSERT OR IGNORE INTO book_visits(deed,machine) VALUES(?1,?2)", (&key, machine))?;
            }
            db.execute("INSERT OR IGNORE INTO book_complete(deed) SELECT deed FROM book_deeds WHERE deed=?1 AND ended IS NOT NULL",[&key])?;
            db.execute("INSERT OR IGNORE INTO book_imports(kind,identity) VALUES('deed',?1)", [key])?;
            Ok(())
        })
        .map_err(io::Error::other)
}

fn copy(database: &Database, table: &'static str, columns: &'static str, rows: Vec<Vec<Value>>) -> io::Result<()> {
    let bytes = rows
        .iter()
        .flatten()
        .map(|v| match v {
            Value::Text(s) => s.len(),
            Value::Blob(b) => b.len(),
            _ => 8,
        })
        .sum();
    database
        .write_now_at(Priority::Background, bytes, move |db| {
            let count = columns.split(',').count();
            let marks = (1..=count).map(|n| format!("?{n}")).collect::<Vec<_>>().join(",");
            let keys = match table {
                "book_deeds" => 1,
                "book_touches" => 2,
                _ => 5,
            };
            let predicate = columns
                .split(',')
                .take(keys)
                .enumerate()
                .map(|(i, k)| format!("{k}=?{}", i + 1))
                .collect::<Vec<_>>()
                .join(" AND ");
            for row in rows {
                db.execute(
                    &format!("INSERT OR IGNORE INTO {table}({columns}) VALUES({marks})"),
                    params_from_iter(&row),
                )?;
                let read = db.query_row(
                    &format!("SELECT {columns} FROM {table} WHERE {predicate}"),
                    params_from_iter(row.iter().take(keys)),
                    |r| values(r, count),
                )?;
                if read != row {
                    return Err(fastctx_storage::Error::new("activity book import did not verify"));
                }
            }
            Ok(())
        })
        .map_err(io::Error::other)
}

pub(super) fn ensure(node: &Node) -> io::Result<()> {
    if ready(&node.book_database)? {
        return Ok(());
    }
    // One background pass and a foreground book reader can meet here.
    let gate = GATES.get(&node.world.path().join("book-import-all"));
    let _importing = gate.lock();
    if ready(&node.book_database)? {
        return Ok(());
    }
    if legacy(node, "book_deeds")? {
        let mut after = String::new();
        loop {
            let keys = node
                .database
                .read_now(|db| {
                    let mut query = db.prepare("SELECT deed FROM book_deeds WHERE deed>?1 ORDER BY deed LIMIT 64")?;
                    Ok(query
                        .query_map([&after], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?)
                })
                .map_err(io::Error::other)?;
            let Some(last) = keys.last() else { break };
            after = last.clone();
            for key in keys {
                deed(node, &key)?;
            }
        }
    }
    if legacy(node, "book_hour")? {
        let mut cursor: (i64, String, String, i64, String) = (i64::MIN, String::new(), String::new(), 0, String::new());
        loop {
            let rows=node.database.read_now(|db| {
                let mut query=db.prepare(&format!("SELECT {HOUR_COLUMNS} FROM book_hour WHERE (at,session,agent,held,account)>(?1,?2,?3,?4,?5) ORDER BY at,session,agent,held,account LIMIT 16"))?;
                Ok(query.query_map(params![cursor.0,cursor.1,cursor.2,cursor.3,cursor.4],|row|values(row,9))?.collect::<std::result::Result<Vec<_>,_>>()?)
            }).map_err(io::Error::other)?;
            let Some(last) = rows.last() else { break };
            let (
                Value::Integer(at),
                Value::Text(session),
                Value::Text(agent),
                Value::Integer(held),
                Value::Text(account),
            ) = (&last[0], &last[1], &last[2], &last[3], &last[4])
            else {
                return Err(io::Error::other("invalid book hour"));
            };
            cursor = (*at, session.clone(), agent.clone(), *held, account.clone());
            copy(&node.book_database, "book_hour", HOUR_COLUMNS, rows)?;
        }
    }
    node.book_database
        .write_now_at(Priority::Background, 32, |db| {
            db.execute("INSERT OR IGNORE INTO book_imports(kind,identity) VALUES('all','')", [])?;
            Ok(())
        })
        .map_err(io::Error::other)
}

pub(crate) fn start(node: &Arc<Node>) {
    if node.book_database.fault().is_some() {
        return;
    }
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = ensure(&node) {
            node.log.line(format_args!("migrating the activity book: {error}"));
        }
    });
}
