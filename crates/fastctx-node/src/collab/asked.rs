//! Each relationship pins its account until its parent conversation is deleted.
//! At most 131,072 relationships of at most 4 KiB each are admitted per parent;
//! these routing identities cannot be folded or expired without misrouting a
//! later message. A full parent refuses new asks, while existing messages work.
use crate::{
    record_migration::errors::invalid,
    records::{self, Source},
};
use fastctx_storage::Priority;
use fastctx_types::{Address, access::HubAccount, layout::ConversationDir};
use std::{
    io::{self, Read},
    path::Path,
};

const FAMILY: &str = "asked-accounts";
const COUNTS: &str = "asked-account-count";
const ROWS: u64 = 131_072;
const BYTES: usize = 4096;

fn key(address: &Address) -> String {
    format!("{}/{}", address.machine.file_name(), address.resident)
}

fn read_source(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fastctx_platform::disk::open(path)?.take(BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "colleague account exceeds 4 KiB; source preserved"));
    }
    let _: HubAccount = serde_json::from_slice(&bytes)?;
    Ok(bytes)
}

fn full() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "this conversation's colleague account records are full")
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let asked = directory.asked();
    records::ensure_each(directory, FAMILY, move |push| {
        let folders = match fastctx_platform::disk::read_dir(&asked) {
            Ok(folders) => folders,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut count = 0;
        for folder in folders {
            let folder = folder?;
            if !folder.file_type()?.is_dir() {
                continue;
            }
            let machine = folder.file_name();
            for entry in fastctx_platform::disk::read_dir(folder.path())? {
                let path = entry?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                count += 1;
                if count > ROWS {
                    return Err(full());
                }
                crate::record_migration::file(&path, push, |push| {
                    let machine = machine.to_str().ok_or_else(|| invalid("colleague machine is not UTF-8"))?;
                    let id = path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| invalid("colleague identity is not UTF-8"))?;
                    id.parse::<fastctx_types::ResidentId>().map_err(|error| invalid(error.to_string()))?;
                    push(Source::new(format!("{machine}/{id}"), path.clone(), read_source(&path)?)?)
                })?;
            }
        }
        Ok(())
    })?;
    if records::read(directory, |rows| rows.exists(COUNTS, "current"))? {
        return Ok(());
    }
    let count = records::read_background(directory, |rows| {
        Ok(rows.connection().query_row(
            "SELECT count(*) FROM conversation_records WHERE conversation=?1 AND family=?2",
            (rows.conversation(), FAMILY),
            |row| row.get::<_, i64>(0),
        )?)
    })?;
    records::write_at(directory, Priority::Background, 64, move |rows| {
        if !rows.exists(COUNTS, "current")? {
            rows.put(COUNTS, "current", &count)?;
        }
        Ok(())
    })
}

pub(super) fn available(directory: &ConversationDir) -> io::Result<()> {
    prepare(directory)?;
    let count: u64 = records::read(directory, |rows| rows.get(COUNTS, "current"))?.unwrap_or(0);
    if count >= ROWS { Err(full()) } else { Ok(()) }
}

pub(super) fn keep(directory: &ConversationDir, colleague: &Address, account: &HubAccount) -> io::Result<()> {
    let bytes = serde_json::to_vec(account)?;
    if bytes.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "colleague account exceeds 4 KiB"));
    }
    prepare(directory)?;
    let key = key(colleague);
    records::write_at(directory, Priority::Interactive, bytes.len() + 64, move |rows| {
        if rows.exists(FAMILY, &key)? {
            return Err(fastctx_storage::Error::new("colleague account already exists"));
        }
        let count = rows.get::<u64>(COUNTS, "current")?.unwrap_or(0);
        if count >= ROWS {
            return Err(fastctx_storage::Error::new(full().to_string()));
        }
        rows.put_raw(FAMILY, &key, &bytes)?;
        rows.put(COUNTS, "current", &(count + 1))
    })
}

pub(super) fn get(directory: &ConversationDir, colleague: &Address) -> io::Result<Option<HubAccount>> {
    prepare(directory)?;
    records::read(directory, |rows| rows.get(FAMILY, &key(colleague)))
}
