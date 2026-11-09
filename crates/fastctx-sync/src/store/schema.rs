//! An inactive shadow schema requiring explicit installation by a service.
//! Importing this module neither opens a database nor claims a world format.

#[path = "schema_tables.rs"]
mod tables;
pub use tables::TABLES;

pub const COMPONENT: &str = "sync.shadow";
pub const VERSION: u32 = 2;
pub const SCHEMA: &str =
    concat!(include_str!("schema.sql"), include_str!("schema_timeline_index.sql"), include_str!("legacy_pins.sql"));

/// Installs the shadow component inside the service's startup transaction.
/// The independently installed legacy carrier is never created twice.
pub fn initialize(db: &fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()> {
    fastctx_storage::versions::apply(
        db,
        COMPONENT,
        &[
            fastctx_storage::versions::Migration { version: 1, sql: include_str!("schema.sql") },
            fastctx_storage::versions::Migration { version: VERSION, sql: include_str!("schema_timeline_index.sql") },
        ],
    )?;
    crate::migration::initialize_carrier(db)?;
    db.execute(
        "INSERT INTO sync_local_head(singleton,sequence) VALUES(1,X'0000000000000000') ON CONFLICT(singleton) DO NOTHING",
        [],
    )?;
    Ok(())
}
