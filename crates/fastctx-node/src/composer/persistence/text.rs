//! Text-only commits append one update and advance small indexed revisions.

use super::*;
use fastctx_types::composer::sync::{Bytes, Update};

pub(super) struct TextWrite {
    key: documents::Key,
    revision: u64,
    updates: Vec<Vec<u8>>,
    migration: Option<Vec<u8>>,
    compact_due: bool,
}
impl TextWrite {
    pub fn prepare(owner: &str, document: &mut state::Document) -> Result<Self, Failure> {
        let mut updates = std::mem::take(&mut document.pending);
        if updates.is_empty() && document.revision == 0 {
            updates.push(document.checkpoint());
        }
        let added = updates.iter().map(Vec::len).sum::<usize>();
        let next_updates = document.tail_updates.saturating_add(updates.len());
        let next_bytes = document.tail_bytes.saturating_add(added);
        let compact_due = document.tail_updates / 128 != next_updates / 128
            || document.tail_bytes / (256 * 1024) != next_bytes / (256 * 1024);
        let migration = document.checkpoint_migration.take();
        let revision = document.revision;
        document.revision = revision.checked_add(updates.len() as u64).ok_or_else(|| failure("revision overflow"))?;
        document.tail_updates = next_updates;
        document.tail_bytes = next_bytes;
        Ok(Self { key: key(owner, &document.id), revision, updates, migration, compact_due })
    }
    pub fn bytes(&self) -> usize {
        self.key.bytes()
            + self.updates.iter().map(Vec::len).sum::<usize>()
            + self.migration.as_ref().map_or(0, Vec::len)
            + if self.compact_due { super::super::compaction::job_bytes(&self.key) } else { 0 }
    }
    pub fn update(&self) -> Result<Update, Failure> {
        Ok(Update {
            document: self.key.name.clone(),
            revision: self.revision + self.updates.len() as u64,
            update: Bytes(match self.updates.as_slice() {
                [update] => update.clone(),
                updates => fastctx_draft::merge(updates).map_err(failure)?,
            }),
        })
    }
    pub fn compact_due(&self) -> bool {
        self.compact_due
    }
    pub fn commit(self, db: &Connection) -> StoredResult<()> {
        let mut revision = self.revision;
        for update in self.updates {
            revision = documents::append(db, &self.key, revision, &update, None)?;
        }
        if let Some(checkpoint) = self.migration {
            journal::checkpoint(db, &self.key, &checkpoint)?;
        }
        if self.compact_due {
            super::super::compaction::submit(db, &self.key)?;
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
    pub fn compact_due(&self) -> bool {
        self.text.compact_due()
    }
    pub fn update(&self) -> Result<Update, Failure> {
        self.text.update()
    }
    pub fn commit(self, db: &Connection) -> StoredResult<()> {
        let changed =
            db.prepare_cached("UPDATE composers SET revision=?2 WHERE owner=?1 AND revision=?3")?.execute(params![
                self.owner,
                i64::try_from(self.revision).map_err(error)?,
                i64::try_from(self.previous).map_err(error)?
            ])?;
        if changed != 1 {
            return Err(error("the composer changed before its text could commit; synchronize it again"));
        }
        self.text.commit(db)
    }
}
