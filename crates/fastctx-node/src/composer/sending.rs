//! A send claims a complete legacy draft or the caller's frozen Yjs replica.
//! The queue entry and the source deletion are still only tentative here.

use super::*;

/// The queue is published whole with every change of it: this many
/// messages wait in it at most.
const QUEUED_AT_MOST: usize = 256;

/// Makes a queue entry of the draft's words, and reserves it to go at once
/// when nothing stands before it. Returns the entry with that reservation.
pub(super) fn capture(
    node: &Node,
    resident: &ResidentId,
    state: &mut Stored,
    from: &MachineName,
    change: &Change,
    captured: Option<capture::Prepared>,
) -> Result<(fastctx_types::NoticeId, Option<Flight>), Failure> {
    if state.queue.len() >= QUEUED_AT_MOST {
        return Err(failure("the queue holds as many messages as it takes; send or remove some of them first"));
    }
    let document = match change {
        Change::Send { epoch } => {
            if state.draft.epoch != *epoch {
                return Err(failure("this draft was already sent"));
            }
            let draft = state.draft.view();
            if !draft.uploads.is_empty() {
                return Err(failure(
                    "the shared draft has unfinished attachment uploads; wait for them or remove them",
                ));
            }
            if draft.text.trim().is_empty() && draft.attachments.is_empty() {
                return Err(failure("the shared draft is empty"));
            }
            super::keeping::took(state);
            state.draft.take()?
        }
        Change::SendDocument { document, epoch, attachments, .. } => {
            if epoch.is_some_and(|epoch| state.draft.epoch != epoch) {
                return Err(failure("this draft was already sent"));
            }
            if state.draft.id != *document {
                return Err(failure("the shared document identity changed; synchronize it before sending"));
            }
            super::keeping::took(state);
            state.draft.take_snapshot(captured.expect("prepared captured send"), attachments)?
        }
        _ => unreachable!("capture only sends draft text"),
    };
    let entry = crate::speech::new_id("queued");
    state.queue.push(Entry {
        id: entry.clone(),
        from: from.clone(),
        document,
        editors: HashSet::new(),
        error: None,
        submitted: None,
        command: false,
    });
    let now = node.residents.now(resident).ok_or_else(|| failure("unknown resident"))?;
    if now.activity != Activity::Busy
        && state.awaiting_turn.is_none()
        && state.sending.is_none()
        && state.pending.is_empty()
    {
        let flight = state.reserve(&entry, false, true)?;
        Ok((entry, Some(flight)))
    } else {
        Ok((entry, None))
    }
}
