//! Stable scalar identities for the old composer protocol. Modern writers use
//! native Yjs updates; this adapter never turns a concurrent edit into a replace.

mod positions;
use super::{Document, Result};
use yrs::{Assoc, IndexedSequence, Text, Transact};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    pub client: u64,
    pub clock: u32,
}
#[derive(Clone, Debug)]
pub struct Character {
    pub id: Id,
    pub text: String,
}
pub struct Edited {
    pub update: Vec<u8>,
    pub inserted: Vec<Character>,
}

impl Document {
    pub fn characters(&self) -> Vec<Character> {
        let mut characters = Vec::with_capacity(self.scalar_len);
        self.visit_characters(|id, character| characters.push(Character { id, text: character.to_string() }));
        characters
    }

    /// Visits visible Unicode scalars and their native identities without
    /// allocating a second per-character representation.
    pub fn visit_characters(&self, mut visit: impl FnMut(Id, char)) {
        let transaction = self.doc.transact();
        self.text.visit_chunks(&transaction, |chunk| {
            if !chunk.deleted
                && let Some(value) = chunk.text
            {
                let mut clock = chunk.id.clock;
                for character in value.chars() {
                    visit(Id { client: chunk.id.client.get(), clock }, character);
                    clock += character.len_utf16() as u32;
                }
            }
        });
    }

    /// Validates the anchor and deletions, returning the remaining scalar count
    /// without modifying text or materializing every character identity.
    pub fn remaining_after(&self, after: Option<Id>, delete: &[Id]) -> Result<usize> {
        Ok(self.scalar_len - positions::resolve(self, after, delete)?.removed)
    }

    /// A deleted anchor still marks its place. Deleting an already deleted
    /// identity is harmless; an identity outside this document is rejected.
    pub fn edit(&mut self, after: Option<Id>, delete: &[Id], insert: &str) -> Result<Edited> {
        let resolved = positions::resolve(self, after, delete)?;
        let mut transaction = self.doc.transact_mut();
        for &(start, length) in resolved.ranges.iter().rev() {
            self.text.remove_range(&mut transaction, start, length);
        }
        let mut inserted = Vec::with_capacity(insert.chars().count());
        if !insert.is_empty() {
            self.text.insert(&mut transaction, resolved.anchor, insert);
            let position =
                self.text.sticky_index(&transaction, resolved.anchor, Assoc::After).expect("inserted character");
            let id = position.id().expect("inserted character identity");
            let mut clock = id.clock;
            for character in insert.chars() {
                inserted.push(Character { id: Id { client: id.client.get(), clock }, text: character.to_string() });
                clock += character.len_utf16() as u32;
            }
        }
        self.scalar_len = self.scalar_len - resolved.removed + inserted.len();
        Ok(Edited { update: transaction.encode_update_v1(), inserted })
    }
}
