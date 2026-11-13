//! The account records in frames a recovering hub took back, written back
//! where the hub keeps them (design §28.38 four, step 3). A frame the hub
//! orders in a conversation's or an account's records scope carries an
//! account record the hub accepted and signed before it was put back; the
//! record is the hub's own word, so it is written as it was, at its own
//! version, never checked again as a device's write would be. Only one
//! newer than the record kept now is written, in version order, each plan
//! and notes revision kept as history before the next goes over it, as the
//! hub does when it accepts a write. The hub, started after, loads them as
//! it loads every record, and its library already holds them.

use super::handback;
use super::{Assets, Owner, scopes};
use fastctx_content::{Record as Item, State};
use fastctx_storage::rusqlite::params;
use fastctx_types::account_records::Record;
use fastctx_types::assets::Readers;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The streams whose records are account records the hub keeps.
const SOURCED: [&str; 6] = ["plan", "history", "goals", "sentinels", "settings", "skills"];

/// How many frames are read at a time.
const AT_ONCE: usize = 64;

/// Writes back the account records of every stream the recovery
/// `recovery` took frames back into and has not written back yet; each
/// stream noted as done once all of its are. Lines for the report.
pub(super) fn repair(dir: &Path, assets: &Assets, recovery: u64) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String, String, String, i64, i64)> = assets.control.read_now(move |c| {
        let mut statement = c.prepare_cached(
            "SELECT owner, scope, stream, tail_next, applied_next FROM recovery_results
             WHERE recovery = ?1 AND repaired = 0 AND applied_next > tail_next",
        )?;
        let rows = statement.query_map([recovery as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?;
    let mut lines = Vec::new();
    for (owner_key, key, stream, from, to) in rows {
        if SOURCED.contains(&stream.as_str()) {
            let Some(owner) = Owner::from_key(&owner_key) else { continue };
            let Some(scope) = scopes::scope(assets, &key).map_err(anyhow::Error::msg)? else { continue };
            match slot_of(dir, &owner, &scope.descriptor.readers, &scope.descriptor.born) {
                Some(slot) => {
                    let library = handback::library_as_is(assets, &scope).map_err(anyhow::Error::msg)?;
                    let records = records_in(&library, &key, &stream, (from as u64, to as u64))?;
                    let written = write_back(&slot, records)?;
                    lines.push(format!("{key} {stream}: wrote back {written} account record revision(s)"));
                }
                None => lines.push(format!("{key} {stream}: the account its records are kept under is not found; not written back")),
            }
        } else {
            lines.push(format!("{key} {stream}: taken back into the library only; where the hub keeps its source is not written"));
        }
        let (k, s) = (key.clone(), stream.clone());
        assets.control.write_now(256, move |c| {
            c.prepare_cached("UPDATE recovery_results SET repaired = 1 WHERE recovery = ?1 AND scope = ?2 AND stream = ?3")?
                .execute(params![recovery as i64, k, s])?;
            Ok(())
        })?;
    }
    Ok(lines)
}

/// Where the records of the scope's account are kept under the hub's
/// directory `dir`: the account it is owned by, or the opener of the body
/// that owns it, as that account was opened then.
fn slot_of(dir: &Path, owner: &Owner, readers: &Readers, born: &BTreeMap<String, String>) -> Option<PathBuf> {
    let (name, opened) = match (owner, readers) {
        (Owner::Account(name, opened), _) => (name.clone(), *opened),
        (Owner::Body(_), Readers::Body { opener, .. }) => {
            let opened = born.get(&format!("account:{opener}"))?.parse::<i64>().ok()?;
            (opener.to_string(), opened)
        }
        _ => return None,
    };
    let account = dir.join("account-records").join(&name);
    // Kept by the nanosecond it was opened at, named by the millisecond.
    let entries = fastctx_platform::disk::read_dir(&account).ok()?;
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(nanos) = file.to_str().and_then(|text| text.parse::<i128>().ok()) else { continue };
        if nanos.div_euclid(1_000_000) == i128::from(opened) {
            return Some(entry.path());
        }
    }
    None
}

/// The account records the frames of `stream` of `scope` from sequence
/// `from` up to `to` carry, read whole: a record too large for a body is
/// read from its object.
fn records_in(library: &fastctx_content::v2::Library, scope: &str, stream: &str, range: (u64, u64)) -> anyhow::Result<Vec<Record>> {
    let (mut at, to) = range;
    let mut records = Vec::new();
    while at < to {
        let frames = library.export(scope, stream, at, AT_ONCE)?;
        let Some(last) = frames.last() else { break };
        let last = fastctx_content::v2::Header::decode(&last.header).map_err(anyhow::Error::msg)?;
        for frame in &frames {
            let header = fastctx_content::v2::Header::decode(&frame.header).map_err(anyhow::Error::msg)?;
            if header.first >= to {
                break;
            }
            for record in fastctx_content::v2::open(&header, &frame.body).map_err(anyhow::Error::msg)? {
                if let Some(record) = record_of(library, &record.record)? {
                    records.push(record);
                }
            }
        }
        at = last.first + u64::from(last.count);
    }
    Ok(records)
}

fn record_of(library: &fastctx_content::v2::Library, item: &Item) -> anyhow::Result<Option<Record>> {
    let State::Is { body, objects } = &item.state else { return Ok(None) };
    let said: serde_json::Value = serde_json::from_slice(body)?;
    if said.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return Ok(serde_json::from_value(said).ok());
    }
    let Some(object) = objects.first() else { return Ok(None) };
    let mut bytes = Vec::with_capacity(object.size as usize);
    while (bytes.len() as u64) < object.size {
        let piece = library.read(&object.hash, bytes.len() as u64, fastctx_content::v2::READ_BYTES)?;
        if piece.is_empty() {
            break;
        }
        bytes.extend_from_slice(&piece);
    }
    Ok(serde_json::from_slice(&bytes).ok())
}

/// Writes each of `records` newer than what `slot` keeps of it, in version
/// order; how many revisions it wrote.
fn write_back(slot: &Path, mut records: Vec<Record>) -> anyhow::Result<usize> {
    records.sort_by(|a, b| {
        (a.metadata.kind.as_str(), a.metadata.key.as_str(), a.metadata.version).cmp(&(
            b.metadata.kind.as_str(),
            b.metadata.key.as_str(),
            b.metadata.version,
        ))
    });
    let mut written = 0;
    for record in records {
        let (kind, key) = (&record.metadata.kind, &record.metadata.key);
        let digest = Sha256::digest(format!("{kind}\0{key}").as_bytes());
        let path = slot.join(format!("{digest:x}.json"));
        let kept = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Record>(&bytes).ok().map(|kept| kept.metadata.version),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if kept.is_some_and(|kept| kept >= record.metadata.version) {
            continue;
        }
        let archive = matches!(kind.as_str(), fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND);
        let history = path.with_extension("versions");
        fastctx_platform::ensure_private_dir(slot)?;
        if archive {
            crate::account_records::history::preserve(&path, &history)?;
        }
        fastctx_platform::publish(&path, &serde_json::to_vec(&record)?, fastctx_platform::Permissions::Private)?;
        if archive && record.metadata.deleted {
            crate::account_records::history::delete(&history)?;
        }
        written += 1;
    }
    Ok(written)
}
