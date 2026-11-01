//! Published file snapshots are imported through the common migration adapter.
//! The small state is exposed only after every ordered input has been verified.
use super::{Handover, handovers};
use crate::records;
use fastctx_storage::Priority;
use fastctx_types::layout::ConversationDir;
use std::io;

pub(super) fn run(directory: &ConversationDir) -> io::Result<()> {
    crate::inbox::storage::initialize(directory)?;
    records::ensure_each(directory, "handover-format-2", |_| {
        loop {
            let old = records::read_background(directory, |rows| {
                let Some((_, value)) = rows.page::<Handover>("handovers", 0, 1)?.into_iter().next() else {
                    return Ok(None);
                };
                let at = rows.modified("handovers", value.id.as_str())?;
                Ok(Some((value, at)))
            })?;
            let Some((old, at)) = old else { break };
            handovers::put(directory, old.clone(), at)?;
            let imported = records::read_background(directory, |rows| {
                handovers::get(rows, &old.id)?
                    .ok_or_else(|| fastctx_storage::Error::new("imported handover is missing"))
            })?;
            if serde_json::to_vec(&old)? != serde_json::to_vec(&imported)? {
                return Err(io::Error::other("imported handover differs from its original snapshot"));
            }
            records::write_at(directory, Priority::Background, 256, move |rows| {
                // Terminal import precedes this step, so settled history never
                // enters the startup work index.
                if old
                    .turn
                    .as_deref()
                    .map(|turn| super::terminals::header(rows, turn))
                    .transpose()?
                    .flatten()
                    .is_some_and(|head| head.effects_complete)
                    || (old.turn.is_none() && old.unresolved.is_none() && !old.attempting)
                {
                    rows.connection().execute(
                        "UPDATE handover_heads SET active=0 WHERE conversation=?1 AND identity=?2",
                        (rows.conversation(), old.id.as_str()),
                    )?;
                }
                rows.delete("handovers", old.id.as_str())?;
                Ok(())
            })?;
        }
        Ok(())
    })
}
