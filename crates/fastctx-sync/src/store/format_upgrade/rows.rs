//! One owning database and service contain at most T0 and its explicit T1 retry.
use fastctx_storage::{
    Error, Result,
    format_upgrade::{Phase, Record, Service},
    rusqlite::{Connection, params},
};

fn phase(value: Phase) -> &'static str {
    match value {
        Phase::Prepared => "prepared",
        Phase::Copying => "copying",
        Phase::Copied => "copied",
        Phase::Complete => "complete",
        Phase::Abandoned => "abandoned",
        Phase::NotRequired => "not_required",
    }
}
fn read_phase(value: &str) -> Result<Phase> {
    match value {
        "prepared" => Ok(Phase::Prepared),
        "copying" => Ok(Phase::Copying),
        "copied" => Ok(Phase::Copied),
        "complete" => Ok(Phase::Complete),
        "abandoned" => Ok(Phase::Abandoned),
        "not_required" => Ok(Phase::NotRequired),
        _ => Err(Error::new("unknown format-upgrade phase")),
    }
}

pub(super) fn current(db: &Connection, service: Service) -> Result<Option<Record>> {
    let mut statement = db.prepare_cached(
        "SELECT format,written_by,previous_format,upgraded_at,attempt,revision,phase,snapshot_at,file_name,copied_pages,reason FROM sync_format_upgrades WHERE service=?1 ORDER BY format DESC,attempt DESC LIMIT 1",
    )?;
    let mut rows = statement.query([service.name()])?;
    let Some(row) = rows.next()? else { return Ok(None) };
    read(row, service).map(Some)
}

pub(super) fn attempts(db: &Connection, service: Service) -> Result<Vec<Record>> {
    current(db, service)?.ok_or_else(|| Error::new("format upgrade was not initialized"))?;
    let mut statement = db.prepare_cached(
        "SELECT format,written_by,previous_format,upgraded_at,attempt,revision,phase,snapshot_at,file_name,copied_pages,reason FROM sync_format_upgrades WHERE service=?1 AND format=?2 ORDER BY attempt LIMIT 3",
    )?;
    let mut rows = statement.query(params![service.name(), service.format()])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(read(row, service)?);
    }
    if records.len() > 2 {
        return Err(Error::new("format upgrade exceeds its two-attempt bound"));
    }
    Ok(records)
}

fn read(row: &fastctx_storage::rusqlite::Row<'_>, service: Service) -> Result<Record> {
    let format: u32 = row.get(0)?;
    let written_by: String = row.get(1)?;
    if format > service.format() {
        return Err(Error::new(format!(
            "the {} database has format {format}, written by Fastctx {written_by}; this version understands {}",
            service.name(),
            service.format()
        )));
    }
    let record = Record {
        service,
        format,
        written_by,
        previous_format: row.get(2)?,
        upgraded_at_millis: row.get(3)?,
        attempt: row.get(4)?,
        revision: super::super::sql::counter(row, 5)?,
        phase: read_phase(&row.get::<_, String>(6)?)?,
        snapshot_at_millis: row.get(7)?,
        file_name: row.get(8)?,
        copied_pages: row.get(9)?,
        reason: row.get(10)?,
    };
    record.validate()?;
    Ok(record)
}

pub(super) fn insert(db: &Connection, record: &Record) -> Result<()> {
    record.validate()?;
    db.prepare_cached("INSERT INTO sync_format_upgrades(service,format,attempt,written_by,previous_format,upgraded_at,revision,phase,snapshot_at,file_name,copied_pages,reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?.execute(params![record.service.name(),record.format,record.attempt,record.written_by,record.previous_format,record.upgraded_at_millis,record.revision.to_be_bytes().as_slice(),phase(record.phase),record.snapshot_at_millis,record.file_name,record.copied_pages,record.reason])?;
    Ok(())
}
pub(super) fn update(db: &Connection, record: &Record) -> Result<()> {
    record.validate()?;
    let changed = db.prepare_cached("UPDATE sync_format_upgrades SET revision=?4,phase=?5,snapshot_at=?6,file_name=?7,copied_pages=?8,reason=?9 WHERE service=?1 AND format=?2 AND attempt=?3")?.execute(params![record.service.name(),record.format,record.attempt,record.revision.to_be_bytes().as_slice(),phase(record.phase),record.snapshot_at_millis,record.file_name,record.copied_pages,record.reason])?;
    if changed != 1 {
        return Err(Error::new("format-upgrade attempt disappeared"));
    }
    Ok(())
}
