//! Rebuild only the old global primary keys. Existing positions remain unchanged.
use crate::conversation_store::schema::quote;
use fastctx_storage::{
    Error, Result,
    rusqlite::Connection,
    versions::{self, Migration},
};

pub(super) fn records(db: &Connection) -> Result<()> {
    let old = super::primary(db, "conversation_records")? == ["position"];
    let sequence = if old { super::sequence(db, "conversation_records")? } else { 0 };
    versions::apply(
        db,
        "node.records",
        &[
            Migration { version: 1, sql: crate::records::SCHEMA },
            Migration { version: 2, sql: crate::records::SCHEMA },
            Migration { version: 3, sql: crate::records::SCHEMA },
        ],
    )?;
    if old {
        rebuild(
            db,
            "conversation_records",
            "position INTEGER NOT NULL, conversation TEXT NOT NULL, family TEXT NOT NULL, identity TEXT NOT NULL, body BLOB NOT NULL, modified TEXT NOT NULL, expires INTEGER, arrival BLOB, category TEXT, PRIMARY KEY(conversation,position), UNIQUE(conversation,family,identity)",
        )?;
        watermark(db, "conversation_records", "records", sequence)?;
    }
    Ok(())
}

pub(super) fn runs(db: &Connection) -> Result<()> {
    let definitions = [
        (
            "run_writes",
            "run-writes",
            "position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL, id TEXT NOT NULL, body BLOB NOT NULL, PRIMARY KEY(conversation,position), UNIQUE(conversation,call,id)",
        ),
        (
            "run_visits",
            "run-visits",
            "position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL, machine TEXT NOT NULL, job TEXT NOT NULL, PRIMARY KEY(conversation,position), UNIQUE(conversation,call,machine,job)",
        ),
        (
            "run_steps",
            "run-steps",
            "position INTEGER NOT NULL, conversation TEXT NOT NULL, call TEXT NOT NULL, id TEXT NOT NULL, body BLOB, fingerprint TEXT NOT NULL, finished INTEGER NOT NULL, machine TEXT NOT NULL, kind TEXT NOT NULL, first INTEGER NOT NULL, last INTEGER NOT NULL, PRIMARY KEY(conversation,position), UNIQUE(conversation,call,id)",
        ),
    ];
    let old = definitions
        .iter()
        .map(|(table, _, _)| super::primary(db, table).map(|key| key == ["position"]))
        .collect::<Result<Vec<_>>>()?;
    versions::apply(
        db,
        "node.run-facts",
        &[
            Migration { version: 1, sql: crate::run_facts::SCHEMA },
            Migration { version: 2, sql: crate::run_facts::SCHEMA },
            Migration { version: 3, sql: crate::run_facts::SCHEMA },
        ],
    )?;
    for ((table, stream, definition), old) in definitions.into_iter().zip(old) {
        if old {
            rebuild(db, table, definition)?;
            watermark(db, table, stream, 0)?;
        }
    }
    db.execute_batch("DROP TABLE IF EXISTS temp.legacy_position_scopes")?;
    Ok(())
}

fn watermark(db: &Connection, table: &str, stream: &str, sequence: i64) -> Result<()> {
    let table = quote(table);
    let high: i64 =
        db.query_row(&format!("SELECT max(coalesce(max(position),0),?1) FROM {table}"), [sequence], |row| row.get(0))?;
    let seeded: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='legacy_position_scopes')",
        [],
        |row| row.get(0),
    )?;
    if !seeded {
        db.execute_batch("CREATE TEMP TABLE legacy_position_scopes(conversation TEXT PRIMARY KEY) WITHOUT ROWID")?;
        let tables = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND EXISTS(SELECT 1 FROM pragma_table_xinfo(sqlite_schema.name) WHERE name='conversation')")?
            .query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        for name in tables {
            db.execute_batch(&format!(
                "INSERT OR IGNORE INTO temp.legacy_position_scopes SELECT DISTINCT conversation FROM {}",
                quote(&name)
            ))?;
        }
    }
    db.execute(
        "INSERT INTO conversation_record_positions(conversation,stream,position)
        SELECT conversation,?1,?2 FROM temp.legacy_position_scopes WHERE true
        ON CONFLICT(conversation,stream) DO UPDATE SET position=max(position,excluded.position)",
        (stream, high),
    )?;
    Ok(())
}

fn rebuild(db: &Connection, table: &str, definition: &str) -> Result<()> {
    let statements=db.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name=?1 AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([table],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    let columns = db
        .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid")?
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let temporary = format!("{table}_conversation_key");
    db.execute_batch(&format!("CREATE TABLE {} ({definition}) WITHOUT ROWID", quote(&temporary)))?;
    let target = db
        .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid")?
        .query_map([&temporary], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if columns != target {
        return Err(Error::new(format!("cannot preserve unknown columns while upgrading {table}")));
    }
    let names = columns.iter().map(|name| quote(name)).collect::<Vec<_>>().join(",");
    db.execute_batch(&format!(
        "INSERT INTO {} ({names}) SELECT {names} FROM {}; DROP TABLE {}; ALTER TABLE {} RENAME TO {}",
        quote(&temporary),
        quote(table),
        quote(table),
        quote(&temporary),
        quote(table)
    ))?;
    for statement in statements {
        db.execute_batch(&statement)?;
    }
    Ok(())
}
