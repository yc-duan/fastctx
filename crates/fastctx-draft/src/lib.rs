//! Shared plain text with Yjs v1 updates and UTF-16 positions on both sides.
//! Storage and delivery belong to the owner: an update is acknowledged only after
//! its original bytes commit. A checkpoint includes unresolved causal updates,
//! so a restart never discards an edit whose predecessor has not arrived yet.

mod identities;
pub use identities::{Character, Edited, Id};

use yrs::updates::{decoder::Decode, encoder::Encode};
use yrs::{Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, TextRef, Transact, Update};

/// Combines v1 updates without constructing a replica or discarding pending data.
pub fn merge(updates: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Result<Vec<u8>> {
    yrs::merge_updates_v1(updates).map_err(error)
}

pub const TEXT: &str = "text";
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(out)
    }
}
impl std::error::Error for Error {}
fn error(what: impl std::fmt::Display) -> Error {
    Error(what.to_string())
}

/// One mutable authority for a text document. There is deliberately no Clone:
/// Yrs clones share their storage, which must not masquerade as a rollback copy.
pub struct Document {
    doc: Doc,
    text: TextRef,
}
impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}
impl Document {
    pub fn new() -> Self {
        let doc = Doc::with_options(Options { offset_kind: OffsetKind::Utf16, ..Options::default() });
        let text = doc.get_or_insert_text(TEXT);
        Self { doc, text }
    }

    pub fn restore(checkpoint: &[u8]) -> Result<Self> {
        let mut document = Self::new();
        document.apply(checkpoint)?;
        Ok(document)
    }

    /// Integrates a possibly repeated or out-of-order v1 update. If integration
    /// fails, the owner reconstructs this document from its durable updates before
    /// accepting more work; decoding errors do not enter a transaction.
    pub fn apply(&mut self, bytes: &[u8]) -> Result<()> {
        let update = Update::decode_v1(bytes).map_err(error)?;
        self.doc.transact_mut().apply_update(update).map_err(error)
    }

    pub fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }
    pub fn len(&self) -> u32 {
        self.text.len(&self.doc.transact())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// The whole retained state, including deletes and pending causal updates.
    /// encode_diff_v1 alone is insufficient for a durable checkpoint.
    pub fn checkpoint(&self) -> Vec<u8> {
        self.doc.transact().encode_state_as_update_v1(&StateVector::default())
    }

    /// Only the state missing from the receiver's vector, plus its required
    /// deletes and pending causal updates. None asks for an initial snapshot.
    pub fn update_for(&self, vector: Option<&[u8]>) -> Result<Vec<u8>> {
        let vector = vector.map(StateVector::decode_v1).transpose().map_err(error)?.unwrap_or_default();
        Ok(self.doc.transact().encode_state_as_update_v1(&vector))
    }

    /// Local compatibility edits use UTF-16 boundaries, as browser textareas do.
    /// Modern peers normally submit their own identity-preserving binary updates.
    pub fn splice(&mut self, start: u32, end: u32, insert: &str) -> Result<Vec<u8>> {
        let text = self.text();
        if start > end || !boundary(&text, start) || !boundary(&text, end) {
            return Err(error("the draft edit is outside its text or splits a UTF-16 character"));
        }
        let mut transaction = self.doc.transact_mut();
        if start != end {
            self.text.remove_range(&mut transaction, start, end - start);
        }
        if !insert.is_empty() {
            self.text.insert(&mut transaction, start, insert);
        }
        Ok(transaction.encode_update_v1())
    }

    /// Captures and removes exactly the text visible on this replica. Concurrent
    /// insertions first received later survive the returned deletion update.
    pub fn take(&mut self) -> Taken {
        let mut transaction = self.doc.transact_mut();
        let text = self.text.get_string(&transaction);
        let length = self.text.len(&transaction);
        if length != 0 {
            self.text.remove_range(&mut transaction, 0, length);
        }
        Taken { text, update: transaction.encode_update_v1() }
    }
}

pub struct Taken {
    pub text: String,
    pub update: Vec<u8>,
}

fn boundary(text: &str, at: u32) -> bool {
    let mut offset = 0u32;
    for character in text.chars() {
        if offset == at {
            return true;
        }
        offset += character.len_utf16() as u32;
        if offset > at {
            return false;
        }
    }
    offset == at
}
