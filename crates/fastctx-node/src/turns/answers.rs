//! Publishing answer debts is independent of whether the resident is archived.
//! Immutable terminals and paid receipts make replay and late readers idempotent.

use crate::{delivery::terminals, records};
use fastctx_storage::Priority;
use fastctx_types::{Address, layout::ConversationDir};
use std::io;

pub(crate) fn create(directory: &ConversationDir, turn: &str, colleague: &Address, label: &str) -> io::Result<()> {
    crate::inbox::storage::initialize(directory)?;
    loop {
        let (key, colleague, label) = (turn.to_string(), colleague.clone(), label.to_string());
        let count = records::write_at(directory, Priority::Normal, 16 * (512 + label.len()), move |rows| {
            let Some(header) = terminals::header(rows, &key)? else { return Ok(0) };
            let readers = terminals::unowed(rows, &key, 16)?;
            for reader in &readers {
                crate::inbox::answers::owe(
                    rows,
                    crate::inbox::answers::Reference {
                        to: reader.clone(),
                        id: crate::speech::fixed_id("answer", &[&colleague.key(), &key, &reader.key()]),
                        at: header.at,
                        colleague: colleague.clone(),
                        label: label.clone(),
                        turn: key.clone(),
                    },
                )?;
                terminals::owed(rows, &key, reader)?;
            }
            Ok(readers.len())
        })?;
        if count == 0 {
            return Ok(());
        }
    }
}
