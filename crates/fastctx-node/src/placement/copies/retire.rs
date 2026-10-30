//! Detach a retiring path before deciding which bytes may be discarded. A
//! metadata-only tombstone keeps watching for late atomic saves at the old
//! path, so a writer racing the move cannot leave an untracked plan behind.

use super::*;

pub(super) fn staged(slot: &Slot) -> PathBuf {
    slot.path.with_file_name(".plan-retiring.md")
}

pub(super) async fn finish(
    node: &Arc<Node>,
    slot: &Slot,
    state: &mut State,
    link: &fastctx_net::link::Link,
) -> Result<(), Failure> {
    let stage = staged(slot);
    if read(&stage)?.is_none() {
        match std::fs::rename(&slot.path, &stage) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        }
    }
    let Some(text) = read(&stage)? else { return Ok(()) };
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    if state.retirement_digest.as_ref() != Some(&digest) {
        let id = fastctx_net::plan_merge::submission(&state.owner, state.retirement_base, &text);
        match link
            .plan_copy(
                &state.account.account,
                Request::Merge {
                    owner: state.owner.clone(),
                    base: state.retirement_base,
                    markdown: text.clone(),
                    submission: Some(id),
                },
            )
            .await
            .map_err(failure)?
        {
            Reply::Page { .. } => {}
            _ => return Err(failure("the hub did not acknowledge the retiring plan")),
        }
    }
    // Native scripts can keep a writable descriptor to the detached inode.
    // Let their tracked jobs finish before removing it (2026-10-30).
    if crate::placement::jobs::busy(node, std::slice::from_ref(&state.owner))? {
        return Err(failure("the old working computer still has jobs; its detached plan waits for them"));
    }
    if read(&stage)?.as_deref() != Some(&text) {
        return Err(failure("the detached plan changed while its final edit was submitted"));
    }
    std::fs::remove_file(stage).map_err(failure)?;
    state.text.clear();
    keep(node, &slot.key, state)?;
    link.follow_record(&state.account.account, fastctx_types::plan::KIND, state.owner.resident.as_str(), false)
        .await
        .map_err(failure)?;
    slot.watching.store(false, Ordering::Release);
    if read(&slot.path)?.is_some() {
        slot.dirty.store(true, Ordering::Release);
    }
    Ok(())
}
