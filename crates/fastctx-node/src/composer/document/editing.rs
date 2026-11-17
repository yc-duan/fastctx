//! Read-only admission precedes ownership transfer, so permanent rejections
//! retain the warm cell. Applying an admitted edit owns all mutable text.

use super::{Alias, CharacterId, Document, Failure, aliases, failure, limits};
use std::collections::HashSet;

struct Edit {
    after: Option<fastctx_draft::Id>,
    delete: Vec<fastctx_draft::Id>,
    deleted: HashSet<fastctx_draft::Id>,
}
impl Document {
    pub fn check_edit(
        &self,
        operation: &str,
        after: &Option<CharacterId>,
        delete: &[CharacterId],
        insert: &str,
        other_metadata: usize,
    ) -> Result<(), Failure> {
        self.plan(operation, after, delete, insert, other_metadata).map(|_| ())
    }
    fn plan(
        &self,
        operation: &str,
        after: &Option<CharacterId>,
        delete: &[CharacterId],
        insert: &str,
        other_metadata: usize,
    ) -> Result<Option<Edit>, Failure> {
        let receipt_bytes = limits::receipt(operation, after.as_ref(), delete, insert);
        // Surviving aliases prevent replay after the receipt retires; writing
        // the replacement receipt must still fit alongside all metadata.
        if self.indexed.contains(operation) {
            limits::write(other_metadata.saturating_add(self.alias_bytes()), operation, 0, 0, receipt_bytes)?;
            return Ok(None);
        }
        let after = after.as_ref().map(|id| self.identity(id)).transpose()?;
        let delete = delete.iter().map(|id| self.identity(id)).collect::<Result<Vec<_>, _>>()?;
        let inserted = limits::insertion(insert)?;
        let before = self.text().character_count();
        let remaining = self.text().remaining_after(after, &delete).map_err(failure)?;
        if inserted != 0 {
            limits::growth(before, remaining.saturating_add(inserted))?;
        }
        let deleted: HashSet<_> = delete.iter().copied().collect();
        let removed = deleted.iter().filter_map(|id| self.indexed.index(*id));
        let mut removed_count = 0;
        let removed_bytes = removed
            .map(|index| {
                removed_count += 1;
                aliases::encoded(&self.aliases[index])
            })
            .sum::<usize>();
        let commas = removed_count.min(self.aliases.len().saturating_sub(1));
        limits::write(
            other_metadata.saturating_add(self.alias_bytes() - removed_bytes - commas),
            operation,
            inserted,
            insert.len(),
            receipt_bytes,
        )?;
        Ok(Some(Edit { after, delete, deleted }))
    }
    pub fn edit(
        &mut self,
        operation: &str,
        after: &Option<CharacterId>,
        delete: &[CharacterId],
        insert: &str,
    ) -> Result<(), Failure> {
        let Some(edit) = self.plan(operation, after, delete, insert, 0)? else { return Ok(()) };
        let edited = self.text_mut().edit(edit.after, &edit.delete, insert).map_err(failure)?;
        // Forget deleted aliases with the edit: stale references fail
        // explicitly, and an expired insertion receipt may then replay.
        self.delete_aliases(edit.deleted);
        self.pending.push(edited.update);
        for (index, character) in edited.inserted.into_iter().enumerate() {
            self.add_alias(Alias {
                old: CharacterId { operation: operation.into(), index },
                client: character.id.client,
                clock: character.id.clock,
            });
        }
        Ok(())
    }
}
