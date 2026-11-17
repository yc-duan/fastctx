//! The account records in frames a recovering hub took back, written back
//! where the hub keeps them (design §28.38 four, step 3). A frame the hub
//! orders in a conversation's or an account's records scope carries an
//! account record the hub accepted and signed before it was put back; the
//! record is the hub's own word, so it is written as it was, at its own
//! version, never checked again as a device's write would be. Only one
//! newer than the record kept now is written, as the frames hold them — in
//! version order, the hub having ordered each record's versions so — plans'
//! and notes' earlier versions kept as history before the next goes over
//! them, as the hub does when it accepts a write; a sentinel's homes are
//! kept as accepting it keeps them. The frames are read a batch at a time:
//! however many a stream took back, only a batch is held at once. The hub,
//! started after, loads the records as it loads every record, and its
//! library already holds them.

use super::handback;
use super::{Assets, Owner, scopes};
use fastctx_content::{Record as Item, State};
use fastctx_net::directory::Directory;
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
/// stream noted as done once all of its are. `directory` is the hub's, as
/// it was put back. Lines for the report.
pub(super) fn repair(dir: &Path, directory: &Directory, assets: &Assets, recovery: u64) -> anyhow::Result<Vec<String>> {
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
            let Some(slot) = slot_of(dir, directory, &owner, &scope.descriptor.readers, &scope.descriptor.born)? else {
                // Not noted as written back: a later reopening finds it again.
                lines.push(format!("{key} {stream}: the account its records are kept under is not found; not written back"));
                continue;
            };
            let library = handback::library_as_is(assets, &scope).map_err(anyhow::Error::msg)?;
            let written = write_back(assets, &library, &key, &stream, (from as u64, to as u64), &slot)?;
            lines.push(format!("{key} {stream}: wrote back {written} account record revision(s)"));
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
/// that owns it, as that account was opened then. An account that kept no
/// record yet has no place made for them: it is made, named by the moment
/// the directory says the account was opened, where that is the one the
/// scope names (codex-f92d833e #2).
fn slot_of(
    dir: &Path,
    directory: &Directory,
    owner: &Owner,
    readers: &Readers,
    born: &BTreeMap<String, String>,
) -> anyhow::Result<Option<PathBuf>> {
    let (name, opened) = match (owner, readers) {
        (Owner::Account(name, opened), _) => (name.clone(), *opened),
        (Owner::Body(_), Readers::Body { opener, .. }) => {
            let Some(opened) = born.get(&format!("account:{opener}")).and_then(|opened| opened.parse::<i64>().ok()) else {
                return Ok(None);
            };
            (opener.to_string(), opened)
        }
        _ => return Ok(None),
    };
    let account = dir.join("account-records").join(&name);
    // Kept by the nanosecond it was opened at, named by the millisecond.
    if let Ok(entries) = fastctx_platform::disk::read_dir(&account) {
        for entry in entries.flatten() {
            let file = entry.file_name();
            let Some(nanos) = file.to_str().and_then(|text| text.parse::<i128>().ok()) else { continue };
            if nanos.div_euclid(1_000_000) == i128::from(opened) {
                return Ok(Some(entry.path()));
            }
        }
    }
    let Some(found) = directory.accounts.iter().find(|found| found.name.as_str() == name) else { return Ok(None) };
    if found.opened.as_millisecond() != opened {
        return Ok(None);
    }
    let slot = account.join(found.opened.as_nanosecond().to_string());
    fastctx_platform::ensure_private_dir(&slot)?;
    Ok(Some(slot))
}

/// Writes back the account records the frames of `stream` of `scope` from
/// sequence `from` up to `to` carry, read a batch at a time, into `slot`;
/// how many revisions it wrote.
fn write_back(
    assets: &Assets,
    library: &fastctx_content::v2::Library,
    scope: &str,
    stream: &str,
    range: (u64, u64),
    slot: &Path,
) -> anyhow::Result<usize> {
    let (mut at, to) = range;
    let mut written = 0;
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
                    written += usize::from(write_one(assets, scope, slot, &record)?);
                }
            }
        }
        at = last.first + u64::from(last.count);
    }
    Ok(written)
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

/// Writes `record` into `slot` if it is newer than what the slot keeps of
/// it; whether it did.
fn write_one(assets: &Assets, scope: &str, slot: &Path, record: &Record) -> anyhow::Result<bool> {
    let (kind, key) = (&record.metadata.kind, &record.metadata.key);
    let digest = Sha256::digest(format!("{kind}\0{key}").as_bytes());
    let path = slot.join(format!("{digest:x}.json"));
    let kept = match fastctx_platform::disk::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Record>(&bytes).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if kept.as_ref().is_some_and(|kept| kept.metadata.version >= record.metadata.version) {
        return Ok(false);
    }
    let archive = matches!(kind.as_str(), fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND);
    let history = path.with_extension("versions");
    // A record deleted keeps no history: one left beside a deletion was a
    // deletion cut short, finished before what comes after it
    // (codex-f92d833e #8).
    if archive && kept.as_ref().is_some_and(|kept| kept.metadata.deleted) {
        crate::account_records::history::delete(&history)?;
    }
    if archive {
        crate::account_records::history::preserve(&path, &history)?;
    }
    fastctx_platform::publish(&path, &serde_json::to_vec(record)?, fastctx_platform::Permissions::Private)?;
    if archive && record.metadata.deleted {
        crate::account_records::history::delete(&history)?;
    }
    // A sentinel's home, kept for good as accepting it keeps it (`mirror`,
    // codex-f92d833e #7).
    if kind == "sentinel"
        && let Some(home) = record.body["definition"]["home"].as_str()
    {
        super::sentinel_homes::record(assets, scope, home).map_err(anyhow::Error::msg)?;
    }
    Ok(true)
}
