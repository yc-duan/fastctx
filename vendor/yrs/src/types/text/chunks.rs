use super::{ItemContent, TextRef};
use crate::{ID, ReadTxn};

/// One retained text item, in document order. Clock lengths always use UTF-16,
/// independently of the document's offset encoding. Deleted content may have
/// been collected, in which case `text` is absent but its identity range remains.
#[derive(Clone, Copy, Debug)]
pub struct TextChunk<'a> {
    pub id: ID,
    pub len: u32,
    pub deleted: bool,
    pub countable: bool,
    pub text: Option<&'a str>,
}

impl TextRef {
    /// Visits retained items once without merging identity ranges or exposing
    /// mutable storage. The transaction keeps all borrowed strings alive.
    pub fn visit_chunks<T: ReadTxn>(&self, _txn: &T, mut visit: impl FnMut(TextChunk<'_>)) {
        let mut current = self.0.start;
        while let Some(item) = current.as_deref() {
            visit(TextChunk {
                id: item.id,
                len: item.len(),
                deleted: item.is_deleted(),
                countable: item.is_countable(),
                text: match &item.content {
                    ItemContent::String(value) => Some(value),
                    _ => None,
                },
            });
            current = item.right;
        }
    }
}
