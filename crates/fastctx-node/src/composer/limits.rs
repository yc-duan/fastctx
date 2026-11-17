//! Reject permanently oversized edits before loading or locking a composer.

use fastctx_types::composer::{
    Change, CharacterId, DOCUMENT_TOO_LONG, MAX_DOCUMENT_CHARACTERS, Mutation, Request, Target,
};
use fastctx_types::ipc::Failure;

pub(super) fn request(request: &Request) -> Result<(), Failure> {
    match request {
        Request::Sync { input } => update(input),
        Request::Mutate { mutation } => validate_mutation(mutation),
        Request::Read => Ok(()),
    }
}

fn validate_mutation(mutation: &Mutation) -> Result<(), Failure> {
    if let Change::Edit { after, delete, insert, .. } = &mutation.change {
        let count = insertion(insert)?;
        write(
            0,
            &mutation.operation,
            count,
            insert.len(),
            receipt(&mutation.operation, after.as_ref(), delete, insert),
        )?;
    }
    if let Change::Withdraw { send } = &mutation.change
        && (send.is_empty() || send.len() > 256)
    {
        return Err(Failure::new("a withdrawal names the operation of the send it takes back"));
    }
    if let Change::SendDocument { snapshot, take, document, .. } = &mutation.change
        && (snapshot.0.len() > fastctx_types::composer::sync::MAX_SNAPSHOT_BYTES
            || document.len() > 256
            || take.as_ref().is_some_and(|take| take.0.len() > fastctx_types::composer::sync::MAX_TAKE_BYTES))
    {
        return Err(Failure::new(DOCUMENT_TOO_LONG));
    }
    Ok(())
}

pub(crate) fn update(input: &fastctx_types::composer::sync::Input) -> Result<(), Failure> {
    let fastctx_types::composer::sync::Input::Update { document, update, .. } = input else {
        let fastctx_types::composer::sync::Input::Mutate { mutation, .. } = input else { unreachable!() };

        if mutation.operation.len() > 256 {
            return Err(Failure::new(DOCUMENT_TOO_LONG));
        }
        return validate_mutation(mutation);
    };
    if update.0.len() > fastctx_types::composer::sync::MAX_UPDATE_BYTES || document.len() > 256 {
        return Err(Failure::new(DOCUMENT_TOO_LONG));
    }
    Ok(())
}

pub(super) fn edit(state: &super::state::Stored, mutation: &Mutation) -> Result<(), Failure> {
    let Change::Edit { target, epoch, after, delete, insert } = &mutation.change else { return Ok(()) };
    let document = match target {
        Target::Draft => Some(&state.draft),
        Target::Queued { id } => state.queue.iter().find(|entry| &entry.id == id).map(|entry| &entry.document),
    };
    let Some(document) = document.filter(|document| document.epoch == *epoch) else { return Ok(()) };
    let other_metadata = state.metadata_bytes;
    document.check_edit(&mutation.operation, after, delete, insert, other_metadata)
}

pub(super) fn insertion(text: &str) -> Result<usize, Failure> {
    let length = text.chars().take(MAX_DOCUMENT_CHARACTERS + 1).count();
    if length > MAX_DOCUMENT_CHARACTERS {
        return Err(Failure::new(DOCUMENT_TOO_LONG));
    }
    Ok(length)
}

pub(super) fn growth(before: usize, after: usize) -> Result<(), Failure> {
    // Existing oversized documents must still accept deletions and causal
    // updates that do not increase their visible text.
    if after > MAX_DOCUMENT_CHARACTERS && after > before {
        return Err(Failure::new(DOCUMENT_TOO_LONG));
    }
    Ok(())
}

/// JSON string bytes excluding quotes, without allocating an encoded copy.
pub(super) fn escaped(text: &str) -> usize {
    text.bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
            0..=31 => 6,
            _ => 1,
        })
        .sum()
}
fn character(id: &CharacterId) -> usize {
    48usize.saturating_add(escaped(&id.operation))
}
pub(super) fn receipt(operation: &str, after: Option<&CharacterId>, delete: &[CharacterId], insert: &str) -> usize {
    delete
        .iter()
        .fold(escaped(operation).saturating_add(escaped(insert)), |bytes, id| bytes.saturating_add(character(id)))
        .saturating_add(after.map_or(0, character))
}

pub(super) fn write(
    aliases: usize,
    operation: &str,
    inserted: usize,
    text_bytes: usize,
    receipt: usize,
) -> Result<(), Failure> {
    // Decimal widths cover 53-bit clients, u32 clocks, and scalar indices.
    // Account for inserted update/checkpoint bytes and ordinary metadata;
    // admission remains authoritative for attachment changes and text checkpoints.
    let index_digits = inserted.saturating_sub(1).checked_ilog10().map_or(1, |n| n as usize + 1);
    let alias = 52usize.saturating_add(16 + 10 + index_digits).saturating_add(escaped(operation));
    let estimate = aliases
        .saturating_add(inserted.saturating_mul(alias))
        .saturating_add(receipt)
        .saturating_add(text_bytes.saturating_mul(2))
        .saturating_add(64 << 10);
    if estimate > fastctx_storage::Priority::Interactive.input_budget() {
        return Err(Failure::new(DOCUMENT_TOO_LONG));
    }
    Ok(())
}
