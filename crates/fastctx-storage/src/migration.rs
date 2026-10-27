//! Resumable receipt migration. Imports commit and are counted before any source
//! is removed. Callers enumerate incrementally and use bounded batches.

use crate::{Database, Error, Result, receipts};
use std::path::PathBuf;
use std::time::Duration;

pub struct Receipt {
    pub path: PathBuf,
    pub key: receipts::Key,
    pub value: Vec<u8>,
    pub keep: Duration,
}

/// Migrates at most 256 files in one transaction. No process may continue writing
/// these source files after migration starts; live writes use the database.
/// Concurrent imports of the same source are harmless and verify the same value.
pub fn receipts(database: &Database, batch: Vec<Receipt>) -> Result<usize> {
    if batch.len() > 256 {
        return Err(Error::new("a migration batch exceeds 256 receipts"));
    }
    let bytes = batch.iter().map(|item| item.key.bytes() + item.value.len()).sum();
    let batch = database.write_now(bytes, move |connection| {
        for item in &batch {
            receipts::import(connection, &item.key, &item.value, item.keep)?;
        }
        Ok(batch)
    })?;
    let count = database.read_now(|connection| {
        let mut found = 0;
        for item in &batch {
            if receipts::get(connection, &item.key)?.is_none() {
                return Err(Error::new("a migrated receipt is missing; its source was preserved"));
            }
            found += 1;
        }
        Ok(found)
    })?;
    for item in batch {
        match std::fs::remove_file(&item.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(count)
}
