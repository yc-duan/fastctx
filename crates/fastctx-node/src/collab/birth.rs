//! The immutable callee birth configuration is one row per conversation, at most
//! 256 KiB. It survives archive/cold recovery and is deleted with the conversation.
use crate::records::{self, Source};
use fastctx_storage::Priority;
use fastctx_types::{collab::Started, layout::ConversationDir};
use std::io::{self, Read};

const FAMILY: &str = "callee-birth";
const BYTES: usize = 256 * 1024;

fn prepare(directory: &ConversationDir) -> io::Result<()> {
    records::ensure_each(directory, FAMILY, |push| {
        let path = directory.callee();
        let file = match fastctx_platform::disk::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(BYTES as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy callee configuration exceeds 256 KiB; source preserved",
            ));
        }
        let _: Started = serde_json::from_slice(&bytes)?;
        push(Source::new("current".into(), path, bytes)?)
    })
}

pub(super) fn keep(directory: &ConversationDir, started: &Started) -> io::Result<()> {
    let bytes = serde_json::to_vec(started)?;
    if bytes.len() > BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "callee configuration exceeds 256 KiB"));
    }
    prepare(directory)?;
    records::write_at(directory, Priority::Interactive, bytes.len(), move |rows| {
        if rows.exists(FAMILY, "current")? {
            return Err(fastctx_storage::Error::new("callee configuration already exists"));
        }
        rows.put_raw(FAMILY, "current", &bytes)
    })
}

pub(super) fn read(directory: &ConversationDir) -> io::Result<Option<Started>> {
    prepare(directory)?;
    records::read(directory, |rows| rows.get(FAMILY, "current"))
}
