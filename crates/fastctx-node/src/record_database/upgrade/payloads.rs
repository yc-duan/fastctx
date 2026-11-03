//! Old manifests were published only after their pieces. Preserve those roots
//! and reconstruct dependency rows before the new collector can run.
use fastctx_storage::{
    Result,
    rusqlite::Connection,
    versions::{self, Migration},
};

pub(super) fn apply(db: &Connection) -> Result<()> {
    let old =
        super::exists(db, "record_payload_manifests")? && !super::column(db, "record_payload_manifests", "complete")?;
    versions::apply(
        db,
        "node.record-payloads",
        &[
            Migration { version: 1, sql: crate::record_payloads::SCHEMA },
            Migration { version: 2, sql: crate::record_payloads::SCHEMA },
        ],
    )?;
    if old {
        db.execute_batch(
            "ALTER TABLE record_payload_manifests ADD COLUMN complete INTEGER NOT NULL DEFAULT 0;
            INSERT OR IGNORE INTO record_payload_members(conversation,manifest,chunk)
            SELECT m.conversation,m.hash,j.value FROM record_payload_manifests m,json_each(CAST(m.body AS TEXT)) j;
            UPDATE record_payload_manifests SET complete=1;",
        )?;
    }
    db.execute_batch("DROP TABLE IF EXISTS record_payload_marks")?;
    Ok(())
}
