//! The old database is an immutable migration source. Typed archive rows retain
//! SQLite values exactly; source rereads and destination reads prove each batch.
use crate::record_migration::{archive, scheduler};
use fastctx_platform::disk;
use fastctx_storage::{
    Database,
    rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value},
};
use std::{
    io::{self, Write},
    path::PathBuf,
};

pub(super) const DEEDS: &str = "deed,job,began,ended,from_machine,session,agent,held,project,account,script,outcome,code,output,went,looked,copied";
pub(super) const TOUCHES: &str = "deed,path,how,added,removed,seen";
pub(super) const HOURS: &str = "at,session,agent,held,account,runs,failed,busy,files";
#[derive(Clone)]
pub(super) struct Context {
    pub source: Database,
    pub target: Database,
    pub world: PathBuf,
    pub topics: std::sync::Arc<crate::topics::Topics>,
}
impl Context {
    pub fn of(node: &crate::Node) -> Self {
        Self {
            source: node.database.clone(),
            target: node.book_database.clone(),
            world: node.world.path().to_path_buf(),
            topics: node.topics.clone(),
        }
    }
}
pub(super) fn values(
    row: &fastctx_storage::rusqlite::Row<'_>,
    count: usize,
) -> fastctx_storage::rusqlite::Result<Vec<Value>> {
    (0..count).map(|i| row.get(i)).collect()
}
pub(super) fn exists(context: &Context, table: &str) -> io::Result<bool> {
    context
        .source
        .read_background_now(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )?)
        })
        .map_err(io::Error::other)
}
pub(super) fn predicate(columns: &str, keys: usize) -> String {
    columns
        .split(',')
        .take(keys)
        .enumerate()
        .map(|(i, column)| format!("{column} IS ?{}", i + 1))
        .collect::<Vec<_>>()
        .join(" AND ")
}
pub(super) fn columns(table: &str) -> (&'static str, usize) {
    match table {
        "book_deeds" => (DEEDS, 1),
        "book_touches" => (TOUCHES, 2),
        "book_hour" => (HOURS, 5),
        _ => unreachable!("known book source"),
    }
}
pub(super) fn read(db: &Connection, table: &str, keys: &[Value]) -> fastctx_storage::Result<Option<Vec<Value>>> {
    let (columns, count) = columns(table);
    Ok(db
        .query_row(
            &format!("SELECT {columns} FROM {table} WHERE {}", predicate(columns, count)),
            params_from_iter(keys),
            |row| values(row, columns.split(',').count()),
        )
        .optional()?)
}
pub(super) fn copy(context: &Context, table: &'static str, rows: &[Vec<Value>]) -> io::Result<()> {
    scheduler::checkpoint()?;
    let (columns, keys) = columns(table);
    let owned = rows.to_vec();
    let bytes = serde_json::to_vec(&encoded(rows))?.len();
    let matches = context
        .target
        .write_now_then_at(
            scheduler::priority(),
            bytes,
            move |db| {
                let marks =
                    (1..=columns.split(',').count()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(",");
                let mut same = true;
                for row in owned {
                    db.execute(
                        &format!("INSERT OR IGNORE INTO {table}({columns}) VALUES({marks})"),
                        params_from_iter(&row),
                    )?;
                    same &= read(db, table, &row[..keys])?.as_ref() == Some(&row);
                }
                Ok(same)
            },
            super::changes::committed(&context.topics),
        )
        .map_err(io::Error::other)?;
    if !matches {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "an old book row conflicts with its destination"));
    }
    let matches = context
        .source
        .read_background_now(|db| {
            for row in rows {
                if read(db, table, &row[..keys])?.as_ref() != Some(row) {
                    return Ok(false);
                }
            }
            Ok(true)
        })
        .map_err(io::Error::other)?;
    if !matches {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "activity book source changed during verification"));
    }
    Ok(())
}
pub(super) fn encoded(rows: &[Vec<Value>]) -> Vec<Vec<serde_json::Value>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    Value::Null => serde_json::json!(["null"]),
                    Value::Integer(value) => serde_json::json!(["integer", value]),
                    Value::Real(value) => serde_json::json!(["real_bits", value.to_bits()]),
                    Value::Text(value) => serde_json::json!(["text", value]),
                    Value::Blob(value) => serde_json::json!(["blob", value]),
                })
                .collect()
        })
        .collect()
}
pub(super) fn valid(table: &str, row: &[Value]) -> bool {
    let (required_text, optional_text, required_integer, optional_integer): (&[usize], &[usize], &[usize], &[usize]) =
        match table {
            "book_deeds" => (&[0, 1, 9, 10, 11, 14], &[4, 5, 6, 8], &[2, 7, 13, 15, 16], &[3, 12]),
            "book_touches" => (&[0, 1, 2], &[], &[5], &[3, 4]),
            "book_hour" => (&[1, 2, 4], &[], &[0, 3, 5, 6, 7, 8], &[]),
            _ => return false,
        };
    required_text.iter().all(|&i| matches!(&row[i], Value::Text(_)))
        && optional_text.iter().all(|&i| matches!(&row[i], Value::Text(_) | Value::Null))
        && required_integer.iter().all(|&i| matches!(&row[i], Value::Integer(_)))
        && optional_integer.iter().all(|&i| matches!(&row[i], Value::Integer(_) | Value::Null))
        && match table {
            "book_deeds" => {
                matches!(&row[0], Value::Text(value) if !value.is_empty())
                    && matches!(&row[11], Value::Text(value) if matches!(value.as_str(), "running"|"done"|"failed"|"stopped"|"lost"))
            }
            "book_touches" => {
                matches!(&row[1], Value::Text(value) if !value.is_empty())
                    && matches!(&row[2], Value::Text(value) if matches!(value.as_str(), "added"|"changed"|"removed"|"copied"))
            }
            _ => true,
        }
}

pub(super) struct Archive {
    path: PathBuf,
    file: std::fs::File,
}
impl Archive {
    pub fn open(context: &Context, identity: &str) -> io::Result<Self> {
        let path = context
            .world
            .join("migration/book")
            .join(format!("{}.jsonl", crate::speech::fixed_id("book", &[identity])));
        fastctx_platform::ensure_private_dir(path.parent().expect("book staging directory"))?;
        Ok(Self { file: disk::create(&path)?, path })
    }
    pub fn append(&mut self, table: &str, rows: &[Vec<Value>]) -> io::Result<()> {
        for row in encoded(rows) {
            serde_json::to_writer(&mut self.file, &serde_json::json!({"table":table,"values":row}))?;
            self.file.write_all(b"\n")?;
        }
        Ok(())
    }
    pub fn finish(self, context: &Context, damage: Option<&str>) -> io::Result<PathBuf> {
        self.file.sync_all()?;
        drop(self.file);
        if let Some(reason) = damage {
            archive::damaged(&context.world, "@book", "activity-book", &self.path, reason)?;
            Ok(self.path)
        } else {
            let signature = archive::fingerprint(&self.path)?;
            let destination = archive::verified_path(&context.world, "@book", &self.path, &signature)?;
            archive::verified(&context.world, "@book", &self.path, &signature)?;
            Ok(destination)
        }
    }
}

pub(super) fn delete_rows(context: &Context, table: &'static str, rows: Vec<Vec<Value>>) -> io::Result<()> {
    let (columns, keys) = columns(table);
    context
        .source
        .write_now_at(scheduler::priority(), rows.len() * 64, move |db| {
            let mut delete = db.prepare_cached(&format!("DELETE FROM {table} WHERE {}", predicate(columns, keys)))?;
            for row in rows {
                delete.execute(params_from_iter(&row[..keys]))?;
            }
            Ok(())
        })
        .map_err(io::Error::other)
}
pub(super) fn touch_page(context: &Context, key: &str, after: Option<&Value>) -> io::Result<Vec<Vec<Value>>> {
    if !exists(context, "book_touches")? {
        return Ok(Vec::new());
    }
    context.source.read_background_now(|db| Ok(db.prepare_cached(&format!("SELECT {TOUCHES} FROM book_touches WHERE deed=?1 AND (?2 IS NULL OR path>?2) ORDER BY path LIMIT 16"))?
        .query_map((key, after), |row| values(row, 6))?.collect::<std::result::Result<Vec<_>, _>>()?)).map_err(io::Error::other)
}
