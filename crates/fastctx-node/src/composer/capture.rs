//! Decode a frozen send once before entering the composer's locks.
use super::*;

pub(super) struct Prepared {
    pub text: String,
    pub deletion: fastctx_draft::DecodedUpdate,
    pub bytes: Vec<u8>,
}

pub(super) fn prepare(request: &Request) -> Result<Option<Prepared>, Failure> {
    let Request::Mutate { mutation } = request else { return Ok(None) };
    let Change::SendDocument { snapshot, take, .. } = &mutation.change else { return Ok(None) };
    let mut frozen = fastctx_draft::Document::restore(&snapshot.0).map_err(failure)?;
    if frozen.character_count() > fastctx_types::composer::MAX_DOCUMENT_CHARACTERS {
        return Err(Failure::new(fastctx_types::composer::DOCUMENT_TOO_LONG));
    }
    let (text, bytes) = match take {
        Some(bytes) => (frozen.text(), bytes.0.clone()),
        // Older native clients freeze the replica with its original identities.
        // Derive only its new visible deletion, never merge its historical data
        // or pending deletes into authority as a side effect of sending.
        None => {
            let taken = frozen.take();
            (taken.text, taken.update)
        }
    };
    let deletion = fastctx_draft::DecodedUpdate::decode(&bytes).map_err(failure)?;
    if deletion.deletion_units().map_err(failure)? > (fastctx_types::composer::MAX_DOCUMENT_CHARACTERS as u64) * 2 {
        return Err(Failure::new(fastctx_types::composer::DOCUMENT_TOO_LONG));
    }
    Ok(Some(Prepared { text, deletion, bytes }))
}
