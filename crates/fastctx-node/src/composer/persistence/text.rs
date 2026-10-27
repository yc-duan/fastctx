//! Text-only commits update one indexed document and the owner's scalar version.
//! Queue metadata is not serialized or rewritten for ordinary typing.

use super::*;
use fastctx_types::composer::sync::{Bytes, Update};

pub(super) struct TextWrite {
    key: documents::Key,
    revision: u64,
    updates: Vec<Vec<u8>>,
    checkpoint: Option<Vec<u8>>,
}
impl TextWrite {
    pub fn prepare(owner: &str, document: &mut state::Document) -> Result<Self, Failure> {
        let mut updates = std::mem::take(&mut document.pending);
        if updates.is_empty() {
            updates.push(document.checkpoint());
        }
        let added = updates.iter().map(Vec::len).sum::<usize>();
        let checkpoint = (document.revision == 0
            || document.tail_updates + updates.len() >= 128
            || document.tail_bytes.saturating_add(added) >= 256 * 1024)
            .then(|| document.checkpoint());
        let revision = document.revision;
        document.revision = revision.checked_add(updates.len() as u64).ok_or_else(|| failure("revision overflow"))?;
        if checkpoint.is_some() {
            document.tail_updates = 0;
            document.tail_bytes = 0;
        } else {
            document.tail_updates += updates.len();
            document.tail_bytes += added;
        }
        Ok(Self { key: key(owner, &document.id), revision, updates, checkpoint })
    }
    pub fn bytes(&self) -> usize {
        self.key.bytes()
            + self.updates.iter().map(Vec::len).sum::<usize>()
            + self.checkpoint.as_ref().map_or(0, Vec::len)
    }
    pub fn update(&self) -> Result<Update, Failure> {
        Ok(Update {
            document: self.key.name.clone(),
            revision: self.revision + self.updates.len() as u64,
            update: Bytes(fastctx_draft::merge(&self.updates).map_err(failure)?),
        })
    }
    pub fn commit(self, transaction: &Connection) -> StoredResult<()> {
        let last = self.updates.len() - 1;
        let mut revision = self.revision;
        for (index, update) in self.updates.into_iter().enumerate() {
            revision = documents::append(
                transaction,
                &self.key,
                revision,
                &update,
                if index == last { self.checkpoint.as_deref() } else { None },
            )?;
        }
        Ok(())
    }
}

pub(in crate::composer) struct Prepared {
    owner: String,
    previous: u64,
    revision: u64,
    text: TextWrite,
    pub bytes: usize,
}

pub(in crate::composer) fn prepare(id: &ResidentId, state: &mut Stored, document: &str) -> Result<Prepared, Failure> {
    if !state.persisted {
        return Err(failure("the composer has no durable metadata"));
    }
    let previous = state.revision;
    let revision = previous.checked_add(1).ok_or_else(|| failure("revision overflow"))?;
    i64::try_from(revision).map_err(failure)?;
    let owner = id.to_string();
    let text = TextWrite::prepare(&owner, state.editable_document(document)?)?;
    state.revision = revision;
    let bytes = owner.len() + text.bytes();
    Ok(Prepared { owner, previous, revision, text, bytes })
}
impl Prepared {
    pub fn update(&self) -> Result<Update, Failure> {
        self.text.update()
    }
    pub fn commit(self, transaction: &Connection) -> StoredResult<()> {
        let changed = transaction.execute(
            "UPDATE composers SET revision=?2 WHERE owner=?1 AND revision=?3",
            params![
                self.owner,
                i64::try_from(self.revision).map_err(error)?,
                i64::try_from(self.previous).map_err(error)?
            ],
        )?;
        if changed != 1 {
            return Err(error("the composer changed before its text could commit; synchronize it again"));
        }
        self.text.commit(transaction)
    }
}
