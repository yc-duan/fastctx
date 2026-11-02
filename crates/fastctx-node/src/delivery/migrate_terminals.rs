//! Preserve the first outcome, settlement time and queue order while replacing
//! legacy aggregate rows. Verification precedes retiring each aggregate.
use super::{Finished, terminals};
use crate::records;
use fastctx_storage::Priority;
use fastctx_types::layout::ConversationDir;
use std::io;

pub(super) fn run(directory: &ConversationDir) -> io::Result<()> {
    let source = directory.clone();
    records::ensure_step(directory, "terminal-format-2", move || {
        let directory = &source;
        let mut after = 0;
        loop {
            let page = records::read_background(directory, |rows| rows.page::<Finished>("terminals", after, 1))?;
            let Some((cursor, mut old)) = page.into_iter().next() else { break };
            after = cursor;
            old.readers.sort();
            old.readers.dedup();
            terminals::publish_blocking(
                directory,
                &old.turn,
                terminals::Initial {
                    end: old.end.clone(),
                    ended_at: old.ended_at,
                    settles_queue: old.settles_queue,
                    imported: Some((old.at, old.queue_order)),
                    usage: None,
                },
                old.readers.clone(),
            )?;
            let (header, end, readers) = terminals::read(directory, &old.turn)?
                .ok_or_else(|| io::Error::other("imported terminal is missing"))?;
            if header.turn != old.turn
                || header.ended_at != old.ended_at
                || header.at != old.at
                || header.settles_queue != old.settles_queue
                || header.queue_order != old.queue_order
                || end != old.end
                || readers != old.readers
            {
                return Err(io::Error::other("imported terminal differs from its legacy outcome"));
            }
            records::write_at(directory, Priority::Background, 256, move |rows| {
                rows.delete("terminals", &old.turn).map(|_| ())
            })?;
        }
        Ok(())
    })
}
