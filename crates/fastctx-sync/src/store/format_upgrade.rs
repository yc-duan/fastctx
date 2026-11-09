//! Closed startup accounting for the database's local format-upgrade attempt.

use super::{boundary, sql};
use fastctx_storage::{
    Database, Error, ErrorKind, Result,
    format_upgrade::{Begin, Journal, Record, Service, Transition},
    rusqlite::Connection,
};

mod rows;

/// Called after schema installation, inside the same startup transaction that
/// publishes the service's format. An existing attempt is never initialized twice.
/// The owning database supplies data-root identity; no account scope is invented.
pub fn prepare_format_upgrade(db: &Connection, begin: Begin) -> Result<Record> {
    if db.is_autocommit() {
        return Err(sql::invalid("format initialization requires its startup transaction"));
    }
    let _boundary = boundary::enter_typed_write(boundary::Role::FormatUpgrade);
    if let Some(record) = rows::current(db, begin.service)? {
        return Ok(record);
    }
    let record = begin.record()?;
    rows::insert(db, &record)?;
    Ok(record)
}

/// Only the node/hub opening and upgrade modules assemble this handle.
#[derive(Clone)]
pub struct SqliteUpgradeJournal {
    db: Database,
    service: Service,
}
impl SqliteUpgradeJournal {
    pub fn new(db: Database, service: Service) -> Result<Self> {
        let journal = Self { db, service };
        journal.current()?;
        Ok(journal)
    }
}
impl Journal for SqliteUpgradeJournal {
    fn current(&self) -> Result<Record> {
        self.db.read_now(|db| {
            rows::current(db, self.service)?.ok_or_else(|| sql::invalid("format upgrade was not initialized"))
        })
    }

    fn attempts(&self) -> Result<Vec<Record>> {
        self.db.read_now(|db| {
            let transaction = db.unchecked_transaction()?;
            let records = rows::attempts(&transaction, self.service)?;
            transaction.commit()?;
            Ok(records)
        })
    }

    fn advance(&self, expected: Record, transition: Transition) -> Result<Record> {
        if expected.service != self.service {
            return Err(sql::invalid("upgrade receipt belongs to another service"));
        }
        let next = expected.advance(&transition)?;
        self.db.write_now(16 * 1024, move |db| {
            let _boundary = boundary::enter_typed_write(boundary::Role::FormatUpgrade);
            let actual = rows::current(db, expected.service)?;
            if actual.as_ref() == Some(&next) {
                return Ok(next);
            }
            if actual.as_ref() != Some(&expected) {
                return Err(Error::with_kind(ErrorKind::Busy, "format-upgrade receipt is no longer current"));
            }
            rows::update(db, &next)?;
            Ok(next)
        })
    }

    fn interrupted(&self, expected: Record) -> Result<Record> {
        if expected.service != self.service {
            return Err(sql::invalid("upgrade receipt belongs to another service"));
        }
        let (abandoned, retry) = expected.interrupted()?;
        let next = retry.as_ref().unwrap_or(&abandoned).clone();
        self.db.write_now(32 * 1024, move |db| {
            let _boundary = boundary::enter_typed_write(boundary::Role::FormatUpgrade);
            let actual = rows::current(db, expected.service)?;
            if actual.as_ref() == Some(&next) {
                return Ok(next);
            }
            if actual.as_ref() != Some(&expected) {
                return Err(Error::with_kind(ErrorKind::Busy, "interrupted upgrade receipt is no longer current"));
            }
            rows::update(db, &abandoned)?;
            if let Some(retry) = retry {
                rows::insert(db, &retry)?;
            }
            Ok(next)
        })
    }
}
