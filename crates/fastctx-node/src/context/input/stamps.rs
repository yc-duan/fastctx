//! Context proofs are immutable point-lookups, not a decoded conversation-wide map.
//! One conversation admits 131,072 inherited proofs, each at most 4 KiB including
//! its key. They are not folded: forgetting a proof would change old displayed
//! messages. They last until conversation deletion; a full fork fails explicitly.
use super::Stamp;
use crate::records::{self, Source};
use fastctx_storage::Priority;
use fastctx_types::layout::ConversationDir;
use serde::{
    Deserializer as _,
    de::{Error as _, MapAccess, Visitor},
};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Read},
};

const FAMILY: &str = "context-input-stamps";
const COUNTS: &str = "context-input-count";
pub(super) const ROWS: usize = 131_072;
const SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const ROW_BYTES: usize = 4096;

fn encoded(key: &str, stamp: &Stamp) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(stamp)?;
    if key.len() + bytes.len() > ROW_BYTES
        || stamp.digest.len() != 64
        || !stamp.digest.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid or oversized context input proof"));
    }
    Ok(bytes)
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let path = directory.books().join("context-inputs.json");
    records::ensure_each(directory, FAMILY, move |push| {
        crate::record_migration::file(&path, push, |push| {
            let file = fastctx_platform::disk::open(&path)?;
            if file.metadata()?.len() > SOURCE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "legacy context proofs exceed 64 MiB; source preserved",
                ));
            }
            struct Import<'a> {
                path: &'a std::path::Path,
                push: &'a mut dyn FnMut(Source) -> io::Result<()>,
            }
            impl<'de> Visitor<'de> for Import<'_> {
                type Value = ();
                fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
                    out.write_str("context input proof map")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
                    let mut count = 0;
                    while let Some((key, stamp)) = map.next_entry::<String, Stamp>()? {
                        count += 1;
                        if count > ROWS {
                            return Err(A::Error::custom(
                                "legacy context proofs exceed 131072 entries; source preserved",
                            ));
                        }
                        let body = encoded(&key, &stamp).map_err(A::Error::custom)?;
                        (self.push)(Source::new(key, self.path.to_path_buf(), body).map_err(A::Error::custom)?)
                            .map_err(A::Error::custom)?;
                    }
                    Ok(())
                }
            }
            let mut decoder = serde_json::Deserializer::from_reader(io::BufReader::new(file.take(SOURCE_BYTES + 1)));
            decoder.deserialize_map(Import { path: &path, push }).map_err(io::Error::other)?;
            decoder.end().map_err(io::Error::other)
        })
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

pub(super) fn get(directory: &ConversationDir, client: &str) -> io::Result<Option<Stamp>> {
    records::read(directory, |rows| rows.get(FAMILY, client))
}

pub(super) fn keep(directory: &ConversationDir, values: BTreeMap<String, Stamp>) -> io::Result<()> {
    if values.len() > ROWS {
        return Err(io::Error::new(io::ErrorKind::StorageFull, "too many inherited context input proofs"));
    }
    prepare(directory)?;
    let mut entries = values.into_iter();
    loop {
        let batch = entries
            .by_ref()
            .take(16)
            .map(|(key, stamp)| encoded(&key, &stamp).map(|body| (key, body)))
            .collect::<io::Result<Vec<_>>>()?;
        if batch.is_empty() {
            return Ok(());
        }
        let bytes = batch.iter().map(|(key, body)| key.len() + body.len() + 64).sum();
        records::write_at(directory, Priority::Interactive, bytes, move |rows| {
            let mut count = rows.get::<usize>(COUNTS, "current")?.unwrap_or(0);
            for (key, body) in batch {
                if let Some(previous) = rows.get_raw(FAMILY, &key)? {
                    if previous != body {
                        return Err(fastctx_storage::Error::new("an inherited context proof changed"));
                    }
                    continue;
                }
                if count >= ROWS {
                    return Err(fastctx_storage::Error::new("too many inherited context input proofs"));
                }
                rows.put_raw(FAMILY, &key, &body)?;
                count += 1;
            }
            rows.put(COUNTS, "current", &count)
        })?;
    }
}
