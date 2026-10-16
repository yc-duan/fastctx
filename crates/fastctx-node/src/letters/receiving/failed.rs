//! A legacy letter too large to deliver remains one recoverable failure, with
//! its original JSON and every acknowledged tuple retained together.

use std::io;
use std::path::Path;

use fastctx_platform::{Permissions, Staged, publish};
use serde::Serialize;

#[derive(Serialize)]
struct Failed<'a> {
    reason: String,
    sender_key: &'a fastctx_net::sign::PublicKey,
    original_id: &'a str,
    business_id: &'a fastctx_types::NoticeId,
    whole: &'static str,
    fragments: &'a Path,
    parts: usize,
    bytes: u64,
}

pub(super) fn keep(directory: &Path, identity: &super::Identity, total: usize, limit: usize) -> io::Result<String> {
    let _memory =
        fastctx_net::reassembly::Reservation::new(super::STORED_BYTES + super::PART_BYTES).map_err(io::Error::other)?;
    let target = directory.join("whole.json");
    let partial = directory.join(".whole-partial");
    let (mut whole, held) = Staged::resume(&target, partial.clone(), Some(0o600))?;
    if held != 0 {
        whole.discard();
        (whole, _) = Staged::resume(&target, partial, Some(0o600))?;
    }
    let mut bytes = 0_u64;
    for part in 0..total {
        let piece = super::stored(directory, part, total)?
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidData, reason))?;
        whole.write_all(piece.as_bytes())?;
        bytes = bytes
            .checked_add(piece.len() as u64)
            .ok_or_else(|| io::Error::other("the archived letter's length overflows"))?;
    }
    whole.commit()?;
    let failure = directory.join("failure.json");
    let (sender_key, original_id) = identity.source();
    let record = Failed {
        reason: format!("this legacy letter has {bytes} bytes, exceeding the {limit}-byte delivery limit"),
        sender_key,
        original_id,
        business_id: &identity.id,
        whole: "whole.json",
        fragments: directory,
        parts: total,
        bytes,
    };
    publish(&failure, &serde_json::to_vec_pretty(&record).map_err(io::Error::other)?, Permissions::Private)?;
    Ok(format!(
        "{}; the complete original and all fragments are preserved; recovery record: {}",
        record.reason,
        failure.display()
    ))
}
