//! Hub answers as whole frames or JSON text parts that fit the reader's limit.

use fastctx_net::wire::{self, Down, Told};

/// The answer to the question `id` as the frames it goes in: one when it
/// fits; else in parts for a member that reads them, or an explicit error.
pub(super) fn frames(id: u64, answer: Result<Told, String>, parts: bool) -> Vec<Down> {
    let whole = Down::Told { id, answer };
    if wire::encode(&whole).len() <= wire::MAX_CONTROL_FRAME {
        return vec![whole];
    }
    let Down::Told { answer, .. } = whole else { unreachable!("made above") };
    let text = serde_json::to_string(&answer).expect("answers serialize");
    let too_large = |why: String| vec![Down::Told { id, answer: Err(why) }];
    if !parts {
        return too_large(format!(
            "the hub's answer is {} bytes, more than this release of Fastctx reads in one frame; update it",
            text.len()
        ));
    }
    if text.len() > wire::PARTS_AT_MOST {
        return too_large(format!(
            "the hub's answer is {} bytes, more than {} it sends in parts",
            text.len(),
            wire::PARTS_AT_MOST
        ));
    }
    // The total is not known yet; reserve the widest possible counters.
    let overhead = wire::encode(&Down::Part { id, index: u32::MAX, total: u32::MAX, text: String::new() }).len();
    let room = wire::MAX_CONTROL_FRAME - overhead;
    let mut pieces = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let end = prefix(rest, room);
        let (piece, after) = rest.split_at(end);
        pieces.push(piece.to_string());
        rest = after;
    }
    let total = u32::try_from(pieces.len()).expect("parts are few");
    pieces.into_iter().enumerate().map(|(index, text)| Down::Part { id, index: index as u32, total, text }).collect()
}

/// A UTF-8 prefix whose JSON string contents take at most `room` bytes.
fn prefix(text: &str, room: usize) -> usize {
    let mut used = 0;
    let mut end = 0;
    for &byte in text.as_bytes().iter().take(wire::PART) {
        // serde_json leaves UTF-8 bytes intact, doubles these escapes, and
        // writes the remaining ASCII controls as six-byte Unicode escapes.
        let size = match byte {
            b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0c' | b'\r' => 2,
            0..=0x1f => 6,
            _ => 1,
        };
        if used + size > room {
            break;
        }
        used += size;
        end += 1;
    }
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    assert!(end > 0, "a part must fit at least one character");
    end
}
