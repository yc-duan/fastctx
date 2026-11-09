//! Additive hub format installation; recovery copying never holds service opening.
use super::shelf::{self, FILE};
use fastctx_storage::{
    Result,
    format_upgrade::{self, BackupBasis, Begin, Event, Options, Phase, Service},
    rusqlite::{Connection, OpenFlags},
};
use fastctx_sync::store::{
    format_upgrade::{SqliteUpgradeJournal, prepare_format_upgrade},
    schema,
};
use std::{io, path::Path, sync::Arc};

pub(super) const CURRENT: u64 = 3;
pub(super) struct Observation {
    basis: BackupBasis,
    previous: Option<u32>,
    at: i64,
}

/// Called before SQLite can create the file, while the opener owns the hub root.
pub(super) fn prepare(dir: &Path) -> io::Result<Observation> {
    let path = dir.join(FILE);
    let basis = BackupBasis::observe(&path)?;
    let mut previous = None;
    if fastctx_platform::disk::exists(&path)? {
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(io::Error::other)?;
        let known: bool = db
            .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='hub_meta')", [], |row| {
                row.get(0)
            })
            .map_err(io::Error::other)?;
        if known && let Some(format) = shelf::meta::<u64>(&db, "format").map_err(shelf::other)? {
            if format > CURRENT {
                let writer = shelf::meta::<String>(&db, "written_by")
                    .map_err(shelf::other)?
                    .unwrap_or_else(|| "an unrecorded newer version".into());
                format_upgrade::validate_written_by(&writer).map_err(shelf::other)?;
                return Err(io::Error::other(format!(
                    "this hub's database has format {format}, written by Fastctx {writer}; this Fastctx understands {CURRENT}; install {writer} or a newer version"
                )));
            }
            if format < CURRENT {
                previous = Some(u32::try_from(format).map_err(io::Error::other)?);
            }
        }
    }
    Ok(Observation { basis, previous, at: jiff::Timestamp::now().as_millisecond() })
}

/// Runs inside the same short transaction as the hub format publication.
pub(super) fn initialize(db: &Connection, observed: &Observation) -> Result<()> {
    schema::initialize(db)?;
    prepare_format_upgrade(
        db,
        Begin {
            service: Service::Hub,
            previous_format: observed.previous,
            written_by: env!("CARGO_PKG_VERSION").into(),
            backup_basis: observed.basis,
            at_millis: observed.at,
        },
    )?;
    shelf::put_meta(
        db,
        "written_by",
        &serde_json::to_vec(env!("CARGO_PKG_VERSION")).map_err(|e| fastctx_storage::Error::new(e.to_string()))?,
    )
}

pub(super) fn start(database: &fastctx_storage::Database, log: Option<Arc<crate::log::Log>>) -> Result<()> {
    let journal = Arc::new(SqliteUpgradeJournal::new(database.clone(), Service::Hub)?);
    let options = Options {
        observe: Arc::new(move |event| {
            let message = match event {
                Event::Warning(message) => Some(format!("format upgrade: {message}")),
                Event::Finished(record) if record.phase == Phase::Complete => Some(format!(
                    "hub format {} recovery backup T{} complete: {}; snapshot timestamp {} ms",
                    record.format,
                    record.attempt,
                    record.file_name.as_deref().unwrap_or("unknown"),
                    record.snapshot_at_millis.unwrap_or(0)
                )),
                _ => None,
            };
            if let Some(message) = message {
                match &log {
                    Some(log) => log.line(&message),
                    None => eprintln!("{message}"),
                }
            }
        }),
        ..Options::default()
    };
    format_upgrade::start(database, journal, options)?;
    Ok(())
}
