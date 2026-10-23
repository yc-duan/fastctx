//! Bounded upload frames for an immutable ledger append.

use crate::wire::{self, Question, Up};

/// A small question is unchanged. Large ledger writes use the hub's advertised capability.
pub fn frames(id: u64, question: Question, supported: bool) -> Result<Vec<Up>, String> {
    let whole = Up::Ask { id, question };
    if wire::encode(&whole).len() <= wire::MAX_CONTROL_FRAME {
        return Ok(vec![whole]);
    }
    if !supported {
        return Err("this hub is too old to receive large ledger entries; update it".into());
    }
    let Up::Ask { question, .. } = whole else { unreachable!() };
    if !matches!(question, Question::LedgerWrite { .. } | Question::MemoryWrite { .. }) {
        return Err("this question is larger than one control message".into());
    }
    let text = serde_json::to_string(&question).map_err(|error| error.to_string())?;
    if text.len() > wire::PARTS_AT_MOST {
        return Err("the ledger entry exceeds the 16 MiB encoded limit".into());
    }
    // Six is the largest JSON expansion of one byte. This keeps UTF-8 and escapes whole.
    let limit = wire::PART.min((wire::MAX_CONTROL_FRAME - 256) / 6);
    let mut pieces = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut end = rest.len().min(limit);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, after) = rest.split_at(end);
        pieces.push(piece.to_string());
        rest = after;
    }
    let total = pieces.len() as u32;
    Ok(pieces
        .into_iter()
        .enumerate()
        .map(|(index, text)| Up::QuestionPart { id, index: index as u32, total, text })
        .collect())
}
