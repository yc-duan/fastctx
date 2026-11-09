//! Contiguous, bounded prefix audit independent of change classification.
use crate::claude::ingest::{
    checkpoint::{Anchor, Prefix},
    lexical::WINDOW_BYTES,
};
use fastctx_types::sync::{cursor::ByteCount, identity::Digest};
use std::io;

/// Rechecks one saved range, never constructs a hash for bytes not actually read.
/// The caller uses its registered anchor index to supply pages in ascending order.
pub fn advance_prefix(prefix: &Prefix, anchor: &Anchor, bytes: &[u8]) -> io::Result<PrefixCheck> {
    if anchor.offset != prefix.checked_through
        || anchor.bytes == 0
        || anchor.bytes > WINDOW_BYTES as u32
        || bytes.len() != anchor.bytes as usize
    {
        return Err(io::Error::other("prefix audit requires the next complete bounded anchor"));
    }
    let through = anchor
        .offset
        .0
        .checked_add(u64::from(anchor.bytes))
        .ok_or_else(|| io::Error::other("anchor range overflow"))?;
    if anchor.row_boundary.0 > through || anchor.row_boundary < prefix.row_boundary {
        return Err(io::Error::other("anchor row boundary moved outside its checked prefix"));
    }
    if Digest::of(bytes) != anchor.digest {
        return Ok(PrefixCheck::Changed { reparse_from: prefix.row_boundary });
    }
    Ok(PrefixCheck::Matched(Prefix {
        checked_through: ByteCount(through),
        row_boundary: anchor.row_boundary,
        anchor: Some(anchor.clone()),
    }))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixCheck {
    Matched(Prefix),
    Changed { reparse_from: ByteCount },
}
