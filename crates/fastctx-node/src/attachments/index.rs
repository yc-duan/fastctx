//! Attachment use is indexed by the original's identity, independently of its
//! recorded path or current machine name. A conversation keeps at most 16,384
//! pins / 64 MiB, each at most 16 KiB. References cannot be folded
//! or expired: they keep originals alive until the inheriting conversation is
//! deleted. Admission fails explicitly at capacity; existing references remain.

use crate::records;
use fastctx_storage::{Error, Priority, rusqlite::OptionalExtension};
use fastctx_types::{
    ResidentId,
    attachments::Attachment,
    layout::{ConversationDir, Kind},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::Path,
};

const FAMILY: &str = "attachment-pins";
const BUDGET: &str = "attachment-pin-budget";
const MAX_ROWS: u64 = 16_384;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const ROW_BYTES: usize = 16 * 1024;

pub(crate) const SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS attachment_pin_original ON conversation_records(arrival,conversation,identity) WHERE family='attachment-pins';
CREATE UNIQUE INDEX IF NOT EXISTS attachment_direct_pin ON conversation_records(conversation,json_extract(body,'$.id')) WHERE family='attachment-pins' AND category='reference';
";

#[derive(Default, Serialize, Deserialize)]
struct Budget {
    rows: u64,
    bytes: u64,
}

fn encoded(file: &Attachment) -> io::Result<(String, Vec<u8>, Vec<u8>)> {
    let body = serde_json::to_vec(file)?;
    if body.len() > ROW_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "attachment receipt exceeds 16 KiB"));
    }
    let identity = format!("{:x}", Sha256::digest(&body));
    // Preserve the existing lifetime comparison: id, file and home resident.
    let original = Sha256::digest(serde_json::to_vec(&(&file.id, &file.file, &file.conversation().resident))?).to_vec();
    Ok((identity, original, body))
}

fn source(path: &Path) -> io::Result<Option<Attachment>> {
    let file = match fastctx_platform::disk::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(ROW_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > ROW_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "legacy attachment receipt exceeds 16 KiB"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Slot receipts stay where they are, since attachment handling still reads
/// them: the pins are derived from them in place and nothing is archived. A
/// receipt that cannot be read stops the import, because a missing pin could
/// let an original go while this conversation still shows it.
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let source = directory.clone();
    records::ensure_step(directory, FAMILY, move || import(&source))
}

fn import(directory: &ConversationDir) -> io::Result<()> {
    let inherited = directory.attachments().join("inherited.json");
    if fastctx_platform::disk::exists(&inherited)? && fastctx_platform::disk::metadata(&inherited)?.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "legacy inherited attachment list exceeds 64 MiB; source preserved",
        ));
    }
    let mut pending = Vec::new();
    records::legacy::array::<Attachment>(&inherited, None, &mut |file| {
        pending.push((file, "inherited"));
        pin(directory, &mut pending, 16)
    })?;
    let entries = match fastctx_platform::disk::read_dir(directory.attachments()) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        for (name, category) in [("receipt.json", "original"), ("reference.json", "reference")] {
            let path = entry.path().join(name);
            if let Some(file) = source(&path)? {
                let slot = entry.file_name();
                let slot = slot.to_str().ok_or_else(|| io::Error::other("attachment slot is not UTF-8"))?;
                let valid = slot == file.id.as_str()
                    || (category == "original"
                        && slot
                            .strip_prefix(file.id.as_str())
                            .and_then(|tail| tail.strip_prefix('-'))
                            .and_then(|number| number.parse::<u32>().ok())
                            .is_some_and(|number| number >= 2));
                if !valid {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "attachment slot differs from its receipt identity",
                    ));
                }
                pending.push((file, category));
                pin(directory, &mut pending, 16)?;
            }
        }
    }
    pin(directory, &mut pending, 1)?;
    // The budget counts what the import found, which may exceed what admission allows.
    records::write_at(directory, crate::record_migration::scheduler::priority(), 64, |rows| {
        if !rows.exists(BUDGET, "current")? {
            let (count, bytes) = rows.connection().query_row(
                "SELECT count(*),coalesce(sum(length(body)),0) FROM conversation_records WHERE conversation=?1 AND family=?2",
                (rows.conversation(), FAMILY),
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            rows.put(BUDGET, "current", &Budget { rows: count as u64, bytes: bytes as u64 })?;
        }
        Ok(())
    })
}

/// Writes the gathered pins once there are at least `at` of them.
fn pin(directory: &ConversationDir, pending: &mut Vec<(Attachment, &'static str)>, at: usize) -> io::Result<()> {
    if pending.is_empty() || pending.len() < at {
        return Ok(());
    }
    crate::record_migration::scheduler::checkpoint()?;
    let batch = pending
        .drain(..)
        .map(|(file, category)| {
            encoded(&file)
                .map(|(identity, original, body)| (format!("{category}:{identity}"), original, body, category))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let bytes = batch.iter().map(|(_, _, body, _)| body.len() + 256).sum();
    records::write_at(directory, crate::record_migration::scheduler::priority(), bytes, move |rows| {
        for (identity, original, body, category) in batch {
            if !rows.exists(FAMILY, &identity)? {
                rows.put_raw_ordered(FAMILY, &identity, &body, &original)?;
                rows.connection().execute(
                    "UPDATE conversation_records SET category=?4 WHERE conversation=?1 AND family=?2 AND identity=?3",
                    (rows.conversation(), FAMILY, &identity, category),
                )?;
            }
        }
        Ok(())
    })
}

fn insert(
    rows: &records::Rows<'_>,
    identity: &str,
    original: &[u8],
    body: &[u8],
    category: &str,
) -> fastctx_storage::Result<()> {
    let identity = format!("{category}:{identity}");
    if rows.exists(FAMILY, &identity)? {
        return Ok(());
    }
    let mut budget = rows.get::<Budget>(BUDGET, "current")?.unwrap_or_default();
    budget.rows = budget.rows.checked_add(1).ok_or_else(|| Error::new("attachment count exhausted"))?;
    budget.bytes =
        budget.bytes.checked_add(body.len() as u64).ok_or_else(|| Error::new("attachment size exhausted"))?;
    if budget.rows > MAX_ROWS || budget.bytes > MAX_BYTES {
        fastctx_platform::readings::counter("node.attachments.at_capacity").add(1);
        return Err(Error::new(FULL));
    }
    rows.put_raw_ordered(FAMILY, &identity, body, original)?;
    rows.connection().execute(
        "UPDATE conversation_records SET category=?4 WHERE conversation=?1 AND family=?2 AND identity=?3",
        (rows.conversation(), FAMILY, identity, category),
    )?;
    rows.put(BUDGET, "current", &budget)
}

/// Said when a conversation's attachment references can take no more.
pub(super) const FULL: &str = "this conversation's attachment references are full; existing originals remain available";

/// Whether `file` can be pinned here as an original: it is already, or the
/// conversation's references have room for it. Asked before an upload puts
/// anything in place, so that a full conversation refuses it whole rather
/// than keeping an original and a receipt nothing points to.
pub(super) fn admits(directory: &ConversationDir, file: &Attachment) -> io::Result<bool> {
    let (identity, _, body) = encoded(file)?;
    prepare(directory)?;
    records::read(directory, move |rows| {
        if rows.exists(FAMILY, &format!("original:{identity}"))? {
            return Ok(true);
        }
        let budget = rows.get::<Budget>(BUDGET, "current")?.unwrap_or_default();
        Ok(budget.rows < MAX_ROWS && budget.bytes.saturating_add(body.len() as u64) <= MAX_BYTES)
    })
}

pub(super) fn put(directory: &ConversationDir, file: &Attachment, reference: bool) -> io::Result<()> {
    let (identity, original, body) = encoded(file)?;
    prepare(directory)?;
    let id = file.id.to_string();
    records::write_at(directory, Priority::Interactive, body.len() + 256, move |rows| {
        if reference {
            let old: Option<(String, i64)> = rows.connection().query_row("SELECT identity,length(body) FROM conversation_records WHERE conversation=?1 AND family='attachment-pins' AND category='reference' AND json_extract(body,'$.id')=?2 AND identity!=?3", (rows.conversation(), &id, format!("reference:{identity}")), |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            if let Some((old, bytes)) = old {
                let mut budget = rows.get::<Budget>(BUDGET, "current")?.unwrap_or_default();
                budget.rows = budget.rows.saturating_sub(1);
                budget.bytes = budget.bytes.saturating_sub(bytes as u64);
                rows.delete(FAMILY, &old)?;
                rows.put(BUDGET, "current", &budget)?;
            }
        }
        insert(rows, &identity, &original, &body, if reference { "reference" } else { "original" })
    })
}

pub(super) fn reference(directory: &ConversationDir, file: &Attachment) -> io::Result<Option<Attachment>> {
    let (identity, _, _) = encoded(file)?;
    prepare(directory)?;
    records::read(directory, |rows| {
        let direct = rows.connection().query_row("SELECT body FROM conversation_records WHERE conversation=?1 AND family='attachment-pins' AND category='reference' AND json_extract(body,'$.id')=?2", (rows.conversation(),file.id.as_str()), |row| row.get::<_,Vec<u8>>(0)).optional()?;
        if let Some(body) = direct {
            let kept: Attachment = records::decode(&body)?;
            return if kept == *file {
                Ok(Some(kept))
            } else {
                Err(Error::new("the attachment differs from its registered original"))
            };
        }
        let body = rows.connection().query_row("SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3 AND category='inherited'", (rows.conversation(), FAMILY, format!("inherited:{identity}")), |row| row.get::<_,Vec<u8>>(0)).optional()?;
        body.as_deref().map(records::decode).transpose()
    })
}

pub(super) fn listed(directory: &ConversationDir) -> io::Result<Vec<Attachment>> {
    prepare(directory)?;
    records::read_background(directory, |rows| {
        let budget = rows.get::<Budget>(BUDGET, "current")?.unwrap_or_default();
        if budget.rows > MAX_ROWS || budget.bytes > MAX_BYTES {
            return Err(Error::new("legacy attachment list exceeds the bounded reply; originals are preserved"));
        }
        rows.all::<Attachment>(FAMILY)
    })
    .and_then(|files| {
        let mut known = std::collections::HashSet::new();
        let mut distinct = Vec::new();
        for file in files {
            if known.insert(encoded(&file)?.0) {
                distinct.push(file);
            }
        }
        Ok(distinct)
    })
}

/// The caller owns the source's management lease until these commits finish.
/// Every target row is copied from the source in the same transaction, so the
/// caller's history cannot invent a receipt or race replacement of that receipt.
pub(super) fn inherit(source: &ConversationDir, target: &ConversationDir, retained: &[Attachment]) -> io::Result<()> {
    prepare(source)?;
    prepare(target)?;
    let (_, source) = records::locate(source)?;
    let mut keys = std::collections::BTreeSet::new();
    for file in retained {
        keys.insert(encoded(file)?.0);
        if keys.len() > MAX_ROWS as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "inherited attachments exceed 16,384 distinct receipts",
            ));
        }
    }
    for group in keys.into_iter().collect::<Vec<_>>().chunks(16) {
        let (source, keys) = (source.clone(), group.to_vec());
        records::write_at(target, Priority::Interactive, keys.len() * ROW_BYTES, move |rows| {
            for key in keys {
                let value = rows.connection().query_row("SELECT arrival,body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity IN (?3,?4,?5) LIMIT 1", (&source, FAMILY, format!("original:{key}"), format!("reference:{key}"), format!("inherited:{key}")), |row| Ok((row.get::<_,Vec<u8>>(0)?, row.get::<_,Vec<u8>>(1)?))).optional()?;
                if let Some((original, body)) = value {
                    insert(rows, &key, &original, &body, "inherited")?;
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub(crate) fn prepare_all(node: &crate::Node) -> io::Result<ConversationDir> {
    let scope = ConversationDir::at(&node.world.conversations().join("@node"));
    let world = node.world.clone();
    records::ensure_step(&scope, "attachment-reference-index", move || {
        for kept in world.kept()? {
            if matches!(kept.kind, Kind::Home(_)) {
                prepare(&kept.directory)?;
            }
        }
        Ok(())
    })?;
    Ok(scope)
}

/// Finds only candidates for this original, paging the reverse index. Runtime
/// identity membership excludes drafts and deleted residents, as the old scan did.
pub(super) fn reader(
    node: &crate::Node,
    file: &Attachment,
    excluding: Option<&ResidentId>,
    exact: bool,
) -> io::Result<Option<ResidentId>> {
    let (identity, original, _) = encoded(file)?;
    let scope = prepare_all(node)?;
    let mut after = (String::new(), String::new());
    loop {
        let page = candidates(&scope, &identity, &original, &after, exact)?;
        let Some(last) = page.last() else {
            return Ok(None);
        };
        after = last.clone();
        for (id, _) in page {
            let Ok(id) = id.parse::<ResidentId>() else { continue };
            if excluding == Some(&id) || node.residents.deleted(&id).is_some() {
                continue;
            }
            if node.residents.knows(&id) {
                return Ok(Some(id));
            }
            // A failed resident load must not look like permission to delete its originals.
            match crate::residents::read_about(&node.world.conversation(&id)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(_) => return Err(io::Error::other(format!("attachment reader {id} is not available"))),
            }
        }
    }
}

fn candidates(
    directory: &ConversationDir,
    identity: &str,
    original: &[u8],
    after: &(String, String),
    exact: bool,
) -> io::Result<Vec<(String, String)>> {
    records::read_background(directory, |rows| {
        let mut query = rows.connection().prepare_cached("SELECT conversation,identity FROM conversation_records WHERE family='attachment-pins' AND arrival=?1 AND (conversation,identity)>(?2,?3) AND (?4=0 OR identity IN (?5,?6,?7)) ORDER BY conversation,identity LIMIT 128")?;
        Ok(query
            .query_map(
                (
                    &original,
                    &after.0,
                    &after.1,
                    exact,
                    format!("original:{identity}"),
                    format!("reference:{identity}"),
                    format!("inherited:{identity}"),
                ),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?)
    })
}
