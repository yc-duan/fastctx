//! Prepared statements may use only the schema installed before service starts.
//! Row hooks still enforce operation-specific rules when cached statements run.

use crate::{
    Result,
    guard::{self, Action, MigrationPurpose},
};
use rusqlite::{
    Connection,
    hooks::{AuthAction, AuthContext, Authorization},
};
use std::{
    cell::Cell,
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

#[derive(Clone, Copy)]
pub struct Write<'a> {
    pub table: &'a str,
    pub action: Action,
    pub accessor: Option<&'a str>,
    pub migration: Option<MigrationPurpose>,
}

/// A service supplies additional preparation-time restrictions. These checks
/// perform no SQL or external I/O and must also be enforced by its row guard.
pub trait WritePolicy: Send + Sync {
    fn permits(&self, write: Write<'_>) -> bool;
}
pub(crate) struct AllPolicies(pub Vec<Arc<dyn WritePolicy>>);
impl WritePolicy for AllPolicies {
    fn permits(&self, write: Write<'_>) -> bool {
        self.0.iter().all(|policy| policy.permits(write))
    }
}

thread_local! { static CONTROL: Cell<bool> = const { Cell::new(false) }; }
pub(crate) struct TransactionControl(bool);
pub(crate) fn transaction_control(allowed: bool) -> TransactionControl {
    TransactionControl(CONTROL.replace(allowed))
}
impl Drop for TransactionControl {
    fn drop(&mut self) {
        CONTROL.set(self.0);
    }
}

thread_local! { static MAINTENANCE: Cell<bool> = const { Cell::new(false) }; }
pub(crate) struct Maintenance(bool);
pub(crate) fn maintenance() -> Maintenance {
    Maintenance(MAINTENANCE.replace(true))
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        MAINTENANCE.set(self.0);
    }
}

pub(crate) fn tables(connection: &Connection) -> Result<Arc<HashSet<String>>> {
    let mut query =
        connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?;
    Ok(Arc::new(query.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?))
}

pub(crate) fn install(
    connection: &mut Connection,
    tables: Arc<HashSet<String>>,
    writer: bool,
    policy: Option<Arc<dyn WritePolicy>>,
) -> Result<()> {
    let engine: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    connection.flush_prepared_statement_cache();
    connection.authorizer(Some(move |context: AuthContext<'_>| {
        let allowed = catch_unwind(AssertUnwindSafe(|| match context.action {
            AuthAction::Insert { table_name } => {
                writable(&tables, writer, policy.as_deref(), context, table_name, Action::Insert)
            }
            AuthAction::Update { table_name, .. } => {
                writable(&tables, writer, policy.as_deref(), context, table_name, Action::Update)
            }
            AuthAction::Delete { table_name } => {
                writable(&tables, writer, policy.as_deref(), context, table_name, Action::Delete)
            }
            AuthAction::Read { .. } | AuthAction::Select | AuthAction::Recursive => true,
            AuthAction::Transaction { .. } => !writer || CONTROL.get(),
            AuthAction::Savepoint { savepoint_name, .. } => {
                !writer || CONTROL.get() || (!guard::validating() && savepoint_name == "legacy_record_batch")
            }
            AuthAction::Function { function_name } => !function_name.eq_ignore_ascii_case("load_extension"),
            AuthAction::Pragma { pragma_name, pragma_value } => pragma(writer, engine, pragma_name, pragma_value),
            // Runtime schema changes cannot escape row guards (2026-11-08).
            _ => false,
        }))
        .unwrap_or_else(|_| {
            if writer {
                guard::mark_panicked();
            }
            false
        });
        if allowed {
            Authorization::Allow
        } else {
            if writer {
                guard::refuse(crate::Error::new("SQLite operation is outside its registered storage role"));
            }
            Authorization::Deny
        }
    }))?;
    Ok(())
}

fn writable(
    tables: &HashSet<String>,
    writer: bool,
    policy: Option<&dyn WritePolicy>,
    context: AuthContext<'_>,
    table: &str,
    action: Action,
) -> bool {
    writer
        && !guard::validating()
        && context.database_name == Some("main")
        && tables.contains(table)
        && policy.is_none_or(|policy| {
            policy.permits(Write { table, action, accessor: context.accessor, migration: guard::migration() })
        })
}

fn pragma(writer: bool, engine: u32, name: &str, value: Option<&str>) -> bool {
    let name = name.to_ascii_lowercase();
    if [
        "table_info",
        "table_xinfo",
        "index_list",
        "index_info",
        "index_xinfo",
        "foreign_key_list",
        "foreign_key_check",
        "quick_check",
        "integrity_check",
    ]
    .contains(&name.as_str())
    {
        return true;
    }
    if value.is_none()
        && [
            "application_id",
            "user_version",
            "page_count",
            "freelist_count",
            "page_size",
            "journal_mode",
            "synchronous",
            "foreign_keys",
            "wal_autocheckpoint",
            "journal_size_limit",
            "schema_version",
            "data_version",
            "compile_options",
            "cache_size",
            "auto_vacuum",
        ]
        .contains(&name.as_str())
    {
        return true;
    }
    if !writer || guard::validating() {
        return false;
    }
    if name == "user_version" {
        return value.is_some_and(|value| value.parse::<u32>().ok() == Some(engine));
    }
    if !MAINTENANCE.get() {
        return false;
    }
    match name.as_str() {
        "wal_checkpoint" => value.is_none_or(|value| value.eq_ignore_ascii_case("truncate")),
        "synchronous" => {
            value.is_some_and(|value| value.eq_ignore_ascii_case("full") || value.eq_ignore_ascii_case("normal"))
        }
        _ => false,
    }
}
