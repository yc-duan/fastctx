//! Identity and named context at the head of a World tool response.

use crate::{Node, residents::Heard};
use fastctx_types::ResidentId;
use std::io;

pub fn notes(node: &Node, id: &ResidentId) -> io::Result<(Vec<String>, Option<Heard>)> {
    let Some((_, town)) = node.residents.about(id) else { return Ok((Vec::new(), None)) };
    crate::context::refresh_local(node, id)?;
    let (mut notes, mut receipt) = crate::context::collect(node, id)?;
    if node.residents.heard(id).is_none() {
        receipt.preceded_by_identity();
        notes.insert(
            0,
            match &town {
                Some(town) => format!("=== you are {id}, working in the project {town} ==="),
                None => format!("=== you are {id}, in no project ==="),
            },
        );
    }
    Ok((notes, Some(Heard { scratch: None, context: Some(receipt), user_prompt: None, town, ledger: 0 })))
}
