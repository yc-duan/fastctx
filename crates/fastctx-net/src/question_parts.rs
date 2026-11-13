//! Bounded upload frames for an immutable ledger append, and for an asset
//! request too large for one control message: an `Open` that reports every
//! receipt no checkpoint confirmed yet (sync v2 freeze spec, 11.2;
//! codex-5c30d5e9 R02).

use crate::wire::{self, Question, Up};

/// A small question is unchanged. Large ledger writes use the hub's advertised
/// capability; a question sent in parts goes without its name.
pub fn frames(
    id: u64,
    question: Question,
    submission: Option<fastctx_types::submission::Submission>,
    supported: bool,
) -> Result<Vec<Up>, String> {
    let whole = Up::Ask { id, question, submission };
    if wire::encode(&whole).len() <= wire::MAX_CONTROL_FRAME {
        return Ok(vec![whole]);
    }
    if !supported {
        return Err("this hub is too old to receive large ledger entries; update it".into());
    }
    let Up::Ask { question, .. } = whole else { unreachable!() };
    if !matches!(
        question,
        Question::LedgerWrite { .. }
            | Question::MemoryWrite { .. }
            | Question::HistoryWrite { .. }
            | Question::SkillFiles { .. }
            | Question::AccountRecords { .. }
            | Question::OwnedRecords { .. }
            | Question::Assets { .. }
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::assets::Request;

    #[test]
    fn an_asset_request_past_one_control_message_goes_in_parts() {
        let question = Question::Assets { request: Request::Streams { scope: "s".repeat(wire::MAX_CONTROL_FRAME + 1) } };
        let frames = frames(7, question.clone(), None, true).expect("sent in parts");
        assert!(frames.len() > 1, "{} frames", frames.len());
        let mut text = String::new();
        for frame in &frames {
            let Up::QuestionPart { id, text: piece, .. } = frame else { panic!("a part") };
            assert_eq!(*id, 7);
            assert!(wire::encode(frame).len() <= wire::MAX_CONTROL_FRAME, "each part fits one control message");
            text.push_str(piece);
        }
        let whole: Question = serde_json::from_str(&text).expect("the parts make the question");
        assert_eq!(serde_json::to_string(&whole).unwrap(), serde_json::to_string(&question).unwrap());
    }
}
