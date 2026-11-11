//! Rebuildable service tables. Authoritative rows survive every repair.

use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension},
};

#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub name: &'static str,
    pub kind: &'static str,
    pub owner: &'static str,
    pub sources: &'static str,
    pub version: u32,
    pub rebuild: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/derived_tables.rs"));

/// An owned schema object. Keep names stable across versions or include their
/// previous names in `retired` when calling [`ensure`].
pub struct Object {
    pub kind: ObjectKind,
    pub name: &'static str,
    pub sql: &'static str,
}

#[derive(Clone, Copy)]
pub enum ObjectKind {
    Table,
    Index,
    Trigger,
}
impl ObjectKind {
    fn word(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Index => "index",
            Self::Trigger => "trigger",
        }
    }
}

fn keywords<'a>(mut sql: &'a str, words: &[&str]) -> Option<&'a str> {
    for word in words {
        sql = sql.trim_start();
        let end = sql.find(char::is_whitespace).unwrap_or(sql.len());
        if !sql[..end].eq_ignore_ascii_case(word) {
            return None;
        }
        sql = &sql[end..];
    }
    Some(sql.trim_start())
}

fn sql_definition(sql: &str) -> String {
    let sql = sql.trim().trim_end_matches(';');
    for prefix in ["CREATE TABLE", "CREATE INDEX", "CREATE UNIQUE INDEX", "CREATE TRIGGER"] {
        let words = prefix.split_whitespace().collect::<Vec<_>>();
        if let Some(tail) = keywords(sql, &words) {
            let tail = keywords(tail, &["IF", "NOT", "EXISTS"]).unwrap_or(tail);
            return format!("{prefix} {tail}");
        }
    }
    sql.to_string()
}
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Checks one registered projection inside the owner's schema transaction.
/// A missing object, changed definition or mismatched version rebuilds the
/// projection from its authoritative rows. Runtime and asset schemas retain
/// their separate, forward-only version rules. Returns whether it rebuilt.
pub fn ensure(
    db: &Connection,
    name: &str,
    objects: &[Object],
    retired: &[(ObjectKind, &str)],
    rebuild: &str,
) -> Result<bool> {
    if db.is_autocommit() {
        return Err(Error::new("derived schema recovery requires a transaction"));
    }
    let table = TABLES
        .iter()
        .find(|table| table.name == name)
        .ok_or_else(|| Error::new(format!("unregistered derived table: {name}")))?;
    super::REGISTRY.projection(table.kind).map_err(Error::new)?;
    if objects.iter().filter(|object| matches!(object.kind, ObjectKind::Table)).count() != 1
        || objects.iter().any(|object| matches!(object.kind, ObjectKind::Table) && object.name != name)
        || retired.iter().any(|(kind, _)| matches!(kind, ObjectKind::Table))
    {
        return Err(Error::new("derived schema does not declare its registered table"));
    }
    let component = format!("derived.{name}");
    let version: Option<bool> = db.query_row(
        "SELECT version FROM storage_versions WHERE component=?1", [&component],
        |row| Ok(matches!(row.get_ref(0)?, fastctx_storage::rusqlite::types::ValueRef::Integer(value) if value == i64::from(table.version))),
    ).optional()?;
    let mut current = version == Some(true);
    for object in objects {
        let sql: Option<String> = db
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type=?1 AND name=?2",
                (object.kind.word(), object.name),
                |row| row.get(0),
            )
            .optional()?;
        current &= sql.is_some_and(|sql| sql_definition(&sql) == sql_definition(object.sql));
    }
    if current {
        return Ok(false);
    }
    for (kind, name) in retired.iter().copied().chain(objects.iter().rev().map(|object| (object.kind, object.name))) {
        let word = if matches!(kind, ObjectKind::Table) {
            db.query_row("SELECT type FROM sqlite_schema WHERE name=?1 AND type IN ('table','view')", [name], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_else(|| "table".into())
        } else {
            kind.word().to_string()
        };
        db.execute_batch(&format!("DROP {word} IF EXISTS {}", quoted(name)))?;
    }
    for object in objects {
        db.execute_batch(object.sql)?;
    }
    db.execute_batch(rebuild)?;
    db.execute("INSERT INTO storage_versions(component,version) VALUES(?1,?2) ON CONFLICT(component) DO UPDATE SET version=excluded.version",(&component,table.version))?;
    Ok(true)
}
