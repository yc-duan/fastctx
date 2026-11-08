//! Transitional source preservation, retired only after canonical import succeeds.
//!
//! Inline sources and family selectors are assets awaiting migration. Totals are
//! derived bookkeeping; neither a deadline nor a completed old consumer unpins them.

use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, params},
    versions::{self, Migration},
};
use sha2::{Digest as _, Sha256};

mod guard;
thread_local! { static PRESERVING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
struct Preservation(bool);
impl Drop for Preservation {
    fn drop(&mut self) {
        PRESERVING.set(self.0);
    }
}
fn preserving() -> bool {
    PRESERVING.get()
}

/// The node combines this guard with its existing conversation lifecycle fences.
pub fn guard(db: &Connection) -> Result<Box<dyn fastctx_storage::Guard>> {
    guard::make(db)
}

/// Carrier totals are bookkeeping, not conversation asset writes. Their own
/// guard restricts updates and verifies the deletion that produced them.
pub fn is_summary(change: &fastctx_storage::guard::Change<'_>) -> Result<bool> {
    use fastctx_storage::{guard::Action, rusqlite::types::ValueRef};
    if change.table() != "sync_legacy_pins" {
        return Ok(false);
    }
    if change.action() != Action::Insert && change.before(1)? != ValueRef::Text(b"@totals") {
        return Ok(false);
    }
    if change.action() != Action::Delete && change.after(1)? != ValueRef::Text(b"@totals") {
        return Ok(false);
    }
    Ok(true)
}

pub const SCHEMA: &str = include_str!("store/legacy_pins.sql");

/// Installs the transitional carrier under its independent migration marker.
pub(crate) fn initialize_carrier(db: &Connection) -> Result<()> {
    versions::apply(db, "sync.legacy-pins", &[Migration { version: 1, sql: SCHEMA }])
}

/// Family pins protect undiscovered files before service guards are installed.
pub fn initialize(db: &Connection) -> Result<()> {
    initialize_carrier(db)?;
    for (identity, kind, selector) in [
        ("job-records", "job.facts", "C/jobs/<job>.{json,sh,writes.jsonl,steps.jsonl}"),
        ("job-segments", "job.facts", "C/jobs/<job>.{writes,steps}/**"),
        ("job-output", "job.output", "JobRecord.output"),
        ("job-stderr", "job.output", "JobRecord.stderr"),
        ("archives", "migration.legacy-pins", "W/archive/<version>/**"),
        ("usage", "account.usage", "W/usage/<login>/<day>.jsonl"),
    ] {
        db.execute("INSERT INTO sync_legacy_pins(conversation,source,identity,fingerprint,kind,selector) VALUES('@node','file-selector',?1,'',?2,?3) ON CONFLICT(conversation,source,identity,fingerprint) DO NOTHING", (identity,kind,selector))?;
    }
    Ok(())
}

/// Preserves complete step revisions in the caller's existing transaction.
/// Only the registered run-facts migration adapter calls this entry point.
pub fn preserve_step(db: &Connection, conversation: &str, call: &str, identity: &str, body: &[u8]) -> Result<()> {
    if db.is_autocommit() || body.len() > 8 * 1024 * 1024 {
        return Err(Error::new("legacy preservation needs a transaction and a bounded source record"));
    }
    let _preserving = Preservation(PRESERVING.replace(true));
    let key = serde_json::to_string(&(call, identity)).map_err(|error| Error::new(error.to_string()))?;
    let hash = format!("{:x}", Sha256::digest(body));
    db.prepare_cached("INSERT INTO sync_legacy_pins(conversation,source,identity,fingerprint,kind,body) VALUES(?1,'run-step',?2,?3,'job.facts',?4) ON CONFLICT(conversation,source,identity,fingerprint) DO NOTHING")?
        .execute(params![conversation,key,hash,body])?;
    Ok(())
}

/// Totals use one indexed row per legacy scope, not a scan of retained bodies.
pub fn report(db: &Connection) -> Result<()> {
    publish_totals(totals(db)?);
    Ok(())
}
fn totals(db: &Connection) -> Result<(i64, i64)> {
    Ok(db.query_row("SELECT coalesce(sum(records),0),coalesce(sum(bytes),0) FROM sync_legacy_pins INDEXED BY sync_legacy_pin_totals WHERE source='@totals'", [], |row| Ok((row.get(0)?,row.get(1)?)))?)
}
fn publish_totals((rows, bytes): (i64, i64)) {
    fastctx_platform::readings::gauge("sync.legacy_pins.rows").set(rows);
    fastctx_platform::readings::gauge("sync.legacy_pins.bytes").set(bytes);
}
