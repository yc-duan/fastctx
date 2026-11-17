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

/// What a projection was last rebuilt with, as a number its version row
/// keeps: FNV-1a of the statement's text.
fn fingerprint(sql: &str) -> i64 {
    let hash =
        sql.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3));
    i64::from_ne_bytes(hash.to_ne_bytes())
}

/// Checks one registered projection inside the owner's schema transaction.
/// A missing object, a changed definition, a changed rebuild statement or a
/// mismatched version rebuilds the projection from its authoritative rows:
/// rows derived by an older rule never stay beside ones derived by the new.
/// `retired` names what earlier versions of the projection left, tables
/// included; it is dropped. Runtime and asset schemas retain their separate,
/// forward-only version rules. Returns whether it rebuilt.
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
        || retired.iter().any(|(_, retired)| *retired == name)
    {
        return Err(Error::new("derived schema does not declare its registered table"));
    }
    let component = format!("derived.{name}");
    let version: Option<bool> = db.query_row(
        "SELECT version FROM storage_versions WHERE component=?1", [&component],
        |row| Ok(matches!(row.get_ref(0)?, fastctx_storage::rusqlite::types::ValueRef::Integer(value) if value == i64::from(table.version))),
    ).optional()?;
    let rebuilt_with = format!("{component}.rebuild");
    let same_rebuild: Option<bool> = db
        .query_row("SELECT version FROM storage_versions WHERE component=?1", [&rebuilt_with], |row| {
            Ok(matches!(row.get_ref(0)?, fastctx_storage::rusqlite::types::ValueRef::Integer(value) if value == fingerprint(rebuild)))
        })
        .optional()?;
    let mut current = version == Some(true) && same_rebuild == Some(true);
    for (kind, retired) in retired {
        let left: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 AND (type=?2 OR (?2='table' AND type='view')))",
            (retired, kind.word()),
            |row| row.get(0),
        )?;
        current &= !left;
    }
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
    db.execute(
        "INSERT INTO storage_versions(component,version) VALUES(?1,?2) ON CONFLICT(component) DO UPDATE SET version=excluded.version",
        (&rebuilt_with, fingerprint(rebuild)),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "conversation_timer_headers";
    const OBJECTS: &[Object] = &[Object {
        kind: ObjectKind::Table,
        name: NAME,
        sql: "CREATE TABLE IF NOT EXISTS conversation_timer_headers (value INTEGER NOT NULL);",
    }];

    fn opened() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE storage_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE source (value INTEGER NOT NULL); INSERT INTO source VALUES (1),(2);",
        )
        .unwrap();
        db
    }

    fn ensured(db: &Connection, retired: &[(ObjectKind, &str)], rebuild: &str) -> bool {
        db.execute_batch("BEGIN").unwrap();
        let rebuilt = ensure(db, NAME, OBJECTS, retired, rebuild).unwrap();
        db.execute_batch("COMMIT").unwrap();
        rebuilt
    }

    fn sum(db: &Connection) -> i64 {
        db.query_row("SELECT coalesce(sum(value),0) FROM conversation_timer_headers", [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_changed_rebuild_statement_rebuilds_what_the_old_one_derived() {
        let db = opened();
        let plain = "INSERT INTO conversation_timer_headers SELECT value FROM source;";
        assert!(ensured(&db, &[], plain));
        assert_eq!(sum(&db), 3);
        assert!(!ensured(&db, &[], plain), "nothing changed: the rows stand as they are");
        let doubled = "INSERT INTO conversation_timer_headers SELECT value*2 FROM source;";
        assert!(ensured(&db, &[], doubled));
        assert_eq!(sum(&db), 6, "no row derived by the old statement is left beside the new");
        assert!(!ensured(&db, &[], doubled));
    }

    #[test]
    fn what_an_earlier_version_left_is_dropped() {
        let db = opened();
        let plain = "INSERT INTO conversation_timer_headers SELECT value FROM source;";
        assert!(ensured(&db, &[], plain));
        db.execute_batch("CREATE TABLE earlier_headers (value INTEGER); CREATE INDEX earlier_index ON source(value);")
            .unwrap();
        let retired = [(ObjectKind::Table, "earlier_headers"), (ObjectKind::Index, "earlier_index")];
        assert!(ensured(&db, &retired, plain));
        let left: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name IN ('earlier_headers','earlier_index')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
        assert!(!ensured(&db, &retired, plain));
    }
}
