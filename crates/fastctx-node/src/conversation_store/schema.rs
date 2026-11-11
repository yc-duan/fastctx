//! Conversation ownership is a schema contract, including future record families.
use fastctx_storage::{Result, rusqlite::Connection};

pub(super) const MACHINE_TABLES: &[&str] = &[
    "storage_versions",
    "storage_salvaged",
    // Jobs use unit ownership and are discarded locally, never exported as records.
    "jobs",
    "claude_copies",
    "receipts",
    "receipt_references",
    "receipt_watermarks",
    "sqlite_sequence",
    "node_conversation_deletions",
    "node_conversation_transfers",
    "node_conversation_transfer_progress",
    "node_conversation_import_rows",
    "node_conversation_arrivals",
    "node_conversation_releases",
    "node_record_state",
    "migration_born",
    "book_deeds",
    "book_touches",
    "book_hour",
    "book_deeds_began",
];

#[derive(Clone, Debug)]
pub(crate) struct Table {
    pub name: String,
    pub writable: Vec<String>,
    pub primary: Vec<String>,
}

pub(crate) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(crate) fn tables(db: &Connection) -> Result<Vec<Table>> {
    let names = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut tables = Vec::new();
    for name in names {
        // Foundation tables use scope/object ownership and their own writer
        // guard; legacy export and deletion must not enumerate them.
        // Keep the transitional carrier in the legacy conversation lifecycle.
        if name != "sync_legacy_pins" && fastctx_sync::store::schema::TABLES.contains(&name.as_str()) {
            continue;
        }
        let columns = db
            .prepare(&format!("PRAGMA table_xinfo({})", quote(&name)))?
            .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?, row.get::<_, i64>(6)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column.0 == "conversation") {
            assert!(
                !cfg!(debug_assertions) || MACHINE_TABLES.contains(&name.as_str()),
                "node table {name} needs a conversation column or an explicit machine-level classification"
            );
            if !MACHINE_TABLES.contains(&name.as_str()) {
                return Err(fastctx_storage::Error::new(format!("unclassified node table: {name}")));
            }
            continue;
        }
        let mut primary: Vec<_> = columns.iter().filter(|column| column.1 > 0).collect();
        primary.sort_by_key(|column| column.1);
        tables.push(Table {
            name,
            primary: primary.into_iter().map(|column| column.0.clone()).collect(),
            writable: columns.iter().filter(|column| column.2 == 0).map(|column| column.0.clone()).collect(),
        });
    }
    Ok(tables)
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    fastctx_storage::versions::apply(
        db,
        "node.jobs",
        &[fastctx_storage::versions::Migration { version: 1, sql: fastctx_jobs::SCHEMA }],
    )?;
    db.execute_batch(crate::claude::wake_store::SCHEMA)?;
    db.execute_batch(crate::record_migration::SCHEMA)?;
    db.execute_batch(crate::records::MIGRATION_PROOF_SCHEMA)?;
    db.execute_batch(crate::context::records::SCHEMA)?;
    db.execute_batch(crate::gather::records::SCHEMA)?;
    super::transfer::initialize(db)?;
    db.execute_batch(crate::session_record::MIGRATION_SCHEMA)?;
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='run_calls')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        db.execute_batch(crate::run_facts::RETIREMENT_SCHEMA)?;
    }
    // Deletion watermarks belong to this node, not to the deleted data. They
    // never expire: an offline producer can return after an arbitrary interval.
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS node_conversation_deletions (
        name TEXT PRIMARY KEY, started INTEGER NOT NULL, purged INTEGER NOT NULL DEFAULT 0,
        bytes INTEGER NOT NULL DEFAULT 0, finished INTEGER NOT NULL DEFAULT 0, account BLOB, fork_source BLOB
    ) WITHOUT ROWID;",
    )?;
    for table in ["composers", "composer_records", "documents", "document_updates"] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        let has_column: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo(?1) WHERE name='conversation')",
            [table],
            |row| row.get(0),
        )?;
        if !has_column {
            db.execute_batch(&format!(
                "ALTER TABLE {} ADD COLUMN conversation TEXT GENERATED ALWAYS AS (owner) VIRTUAL",
                quote(table)
            ))?;
        }
    }
    let delete_triggers = db.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND name IN ('input_membership_delete','input_membership_reopened')")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (name, sql) in delete_triggers {
        if !sql.contains("node_conversation_deletions") {
            db.execute_batch(&format!(
                "DROP TRIGGER {}; {}",
                quote(&name),
                sql.replacen(
                    " WHEN ",
                    " WHEN NOT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=OLD.conversation) AND ",
                    1
                )
            ))?;
        }
    }
    // Writes are fenced by the writer's guard (`fences`), not by triggers: a
    // database an older node opened last still has a trigger per fence on
    // every table, and those go.
    let triggers = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='trigger'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<std::collections::HashSet<_>, _>>()?;
    for table in tables(db)? {
        for fence in [
            "reject_deleted_insert",
            "reject_deleted_update",
            "pending_import_insert",
            "pending_import_update",
            "pending_import_delete",
        ] {
            let trigger = format!("{}_{fence}", table.name);
            if triggers.contains(&trigger) {
                db.execute_batch(&format!("DROP TRIGGER {}", quote(&trigger)))?;
            }
        }
        let name = quote(&table.name);
        let indexed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_index_list(?1) i JOIN pragma_index_info(i.name) c WHERE c.seqno=0 AND c.name='conversation')",
            [&table.name], |row| row.get(0),
        )?;
        if !indexed {
            db.execute_batch(&format!(
                "CREATE INDEX IF NOT EXISTS {} ON {name}(conversation)",
                quote(&format!("{}_conversation", table.name))
            ))?;
        }
    }
    Ok(())
}

pub(crate) fn children_first(db: &Connection, mut tables: Vec<Table>) -> Result<Vec<Table>> {
    let mut references = std::collections::HashMap::new();
    for table in &tables {
        let names = db
            .prepare(&format!("PRAGMA foreign_key_list({})", quote(&table.name)))?
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        references.insert(table.name.clone(), names);
    }
    let mut result = Vec::new();
    while !tables.is_empty() {
        let index = tables
            .iter()
            .position(|table| {
                !tables.iter().any(|child| child.name != table.name && references[&child.name].contains(&table.name))
            })
            .ok_or_else(|| fastctx_storage::Error::new("cyclic conversation foreign keys"))?;
        result.push(tables.remove(index));
    }
    Ok(result)
}
