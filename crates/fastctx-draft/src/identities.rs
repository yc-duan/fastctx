//! Stable scalar identities for the old composer protocol. Modern writers use
//! native Yjs updates; this adapter never turns a concurrent edit into a replace.

use super::{Document, Result, error};
use std::collections::HashSet;
use yrs::{Assoc, GetString, IndexedSequence, StickyIndex, Text, Transact};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
fn native(id: Id) -> Result<yrs::ID> {
    if id.client >= 1 << 53 {
        return Err(error("the draft character has an invalid client id"));
    }
    Ok(yrs::ID::new(yrs::block::ClientID::new(id.client), id.clock))
}

impl Document {
    pub fn characters(&self) -> Vec<Character> {
        let mut transaction = self.doc.transact_mut();
        let text = self.text.get_string(&transaction);
        characters(&self.text, &mut transaction, 0, &text)
    }

    /// A deleted anchor still marks its place. Deleting an already deleted
    /// identity is harmless; an identity outside this document is rejected.
    pub fn edit(&mut self, after: Option<Id>, delete: &[Id], insert: &str) -> Result<Edited> {
        let visible = self.characters();
        let mut at = 0;
        let mut anchor = after.is_none().then_some(0);
        let gone: HashSet<_> = delete.iter().copied().collect();
        let mut ranges = Vec::new();
        for character in &visible {
            let length = character.text.encode_utf16().count() as u32;
            if Some(character.id) == after {
                anchor = Some(at + length);
            }
            if gone.contains(&character.id) {
                ranges.push((at, length));
            }
            at += length;
        }
        let mut transaction = self.doc.transact_mut();
        let mut at = match anchor {
            Some(at) => at,
            None => {
                StickyIndex::from_id(native(after.expect("anchor"))?, Assoc::Before)
                    .get_offset(&transaction)
                    .ok_or_else(|| error("the insertion anchor is not in this draft"))?
                    .index
            }
        };
        let value = self.text.get_string(&transaction);
        for id in after.iter().chain(delete.iter()) {
            let offset = StickyIndex::from_id(native(*id)?, Assoc::After)
                .get_offset(&transaction)
                .ok_or_else(|| error("a character is not in this draft"))?;
            if !super::boundary(&value, offset.index) {
                return Err(error("a character identity splits a UTF-16 character"));
            }
        }
        if !super::boundary(&value, at) {
            return Err(error("the insertion anchor splits a UTF-16 character"));
        }
        let removed_before = ranges.iter().map(|&(start, length)| at.saturating_sub(start).min(length)).sum::<u32>();
        for &(start, length) in ranges.iter().rev() {
            self.text.remove_range(&mut transaction, start, length);
        }
        at -= removed_before;
        if !insert.is_empty() {
            self.text.insert(&mut transaction, at, insert);
        }
        let inserted = characters(&self.text, &mut transaction, at, insert);
        Ok(Edited { update: transaction.encode_update_v1(), inserted })
    }
}

fn characters(
    text: &yrs::TextRef,
    transaction: &mut yrs::TransactionMut<'_>,
    mut at: u32,
    value: &str,
) -> Vec<Character> {
    value
        .chars()
        .map(|character| {
            let position = text.sticky_index(transaction, at, Assoc::After).expect("visible character");
            let id = position.id().expect("visible character identity");
            at += character.len_utf16() as u32;
            Character { id: Id { client: id.client.get(), clock: id.clock }, text: character.to_string() }
        })
        .collect()
}
