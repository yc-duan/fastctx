//! Older nodes sometimes kept only an answer in the outbox. Recover that
//! terminal once, paging legacy answers without hydrating new references.
use crate::{Node, inbox::answers::Stored, records};
use fastctx_types::{ResidentId, residents::Notice};
use std::io;

pub(crate) fn run(node: &std::sync::Arc<Node>, resident: &ResidentId) -> io::Result<()> {
    let directory = node.world.conversation(resident);
    super::prepare(&directory)?;
    crate::inbox::storage::initialize(&directory)?;
    let (node, resident, source) = (node.clone(), resident.clone(), directory.clone());
    records::ensure_step(&directory, "terminal-outgoing-import", move || {
        let directory = source;
        let mut after = String::new();
        loop {
            let page =
                records::read_background(&directory, |rows| rows.key_page::<Stored>("outgoing-answers", &after, 1))?;
            let Some((key, answer)) = page.into_iter().next() else { break };
            after = key;
            let Stored::Inline(answer) = answer else { continue };
            if let Notice::Answer { turn, end, .. } = answer.delivery.notice
                && !turn.is_empty()
            {
                let mut readers = node.residents.readers(&resident, &turn);
                readers.push(answer.to);
                super::terminals::publish_blocking(
                    &directory,
                    &turn,
                    super::terminals::Initial {
                        end,
                        ended_at: None,
                        settles_queue: false,
                        imported: None,
                        usage: None,
                    },
                    readers,
                )?;
            }
        }
        Ok(())
    })
}
