//! The World file fence precedes SQL; background copying follows its commit.
use fastctx_storage::{
    Database, Result,
    format_upgrade::{self, BackupBasis, Begin, Event, Options, Phase, Service},
    rusqlite::Connection,
};
use fastctx_sync::store::{
    format_upgrade::{SqliteUpgradeJournal, prepare_format_upgrade},
    schema,
};
use fastctx_types::layout::WorldDir;
use std::{path::Path, sync::Arc};

pub(super) struct Observation {
    basis: BackupBasis,
    previous: Option<u32>,
    at: i64,
}
pub(super) fn observe(world: &WorldDir, path: &Path) -> Result<Observation> {
    // Also covers diagnostic reopen and drills that call open without Node startup.
    if !fastctx_platform::disk::exists(world.path())? {
        fastctx_platform::ensure_private_dir(world.path())?;
    }
    crate::world_format::claim(world)?;
    let basis = BackupBasis::observe(path)?;
    let previous = if basis.needs_backup() { crate::world_format::previous(world)? } else { None };
    let at = jiff::Timestamp::now().as_millisecond();
    Ok(Observation { basis, previous, at })
}
pub(super) fn guard(db: &Connection) -> Result<Box<dyn fastctx_storage::Guard>> {
    Ok(Box::new(fastctx_storage::guard::CompositeGuard::new(vec![
        crate::asset_preservation::guard(db)?,
        fastctx_sync::store::boundary::guard(db)?,
    ])))
}

pub(super) fn initialize(db: &Connection, observed: &Observation) -> Result<()> {
    schema::initialize(db)?;
    prepare_format_upgrade(
        db,
        Begin {
            service: Service::World,
            previous_format: observed.previous,
            written_by: env!("CARGO_PKG_VERSION").into(),
            backup_basis: observed.basis,
            at_millis: observed.at,
        },
    )?;
    Ok(())
}
pub(super) fn start(database: &Database, log: Arc<crate::log::Log>) -> Result<()> {
    if let Some(error) = database.fault() {
        return Err(error.clone());
    }
    let journal = Arc::new(SqliteUpgradeJournal::new(database.clone(), Service::World)?);
    let options = Options {
        observe: Arc::new(move |event| match event {
            Event::Warning(message) => log.line(format_args!("format upgrade: {message}")),
            Event::Finished(record) if record.phase == Phase::Complete => log.line(format_args!(
                "World format {} recovery backup T{} complete: {}; snapshot timestamp {} ms",
                record.format,
                record.attempt,
                record.file_name.as_deref().unwrap_or("unknown"),
                record.snapshot_at_millis.unwrap_or(0)
            )),
            _ => {}
        }),
        ..Options::default()
    };
    format_upgrade::start(database, journal, options)?;
    Ok(())
}
