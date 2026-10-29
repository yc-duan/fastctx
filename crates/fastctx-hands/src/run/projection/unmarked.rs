//! Canonical output already contains everything needed when no note matched.

use super::Spans;
use fastctx_types::run::{OutputPiece, OutputWindow};
use fastctx_types::run_facts::{DisplayPiece, OutputSpan};

pub(super) fn spans(window: &OutputWindow) -> Vec<OutputSpan> {
    let mut spans = Spans::default();
    let mut image = 0;
    for piece in &window.pieces {
        let piece = match piece {
            OutputPiece::Text { first_line, text } => {
                spans.text(None, *first_line, text.clone());
                continue;
            }
            OutputPiece::PartOfLine { line, from, of, text, .. } => DisplayPiece::PartOfLine {
                line: *line,
                from: *from,
                to: *from + text.chars().count() as u64 - 1,
                of: *of,
                text: text.clone(),
            },
            OutputPiece::Omitted { first_line, last_line } => {
                DisplayPiece::Omitted { first_line: *first_line, last_line: *last_line }
            }
            OutputPiece::Image { mime, width, height, .. } => {
                let piece = DisplayPiece::Image { image, mime: mime.clone(), width: *width, height: *height };
                image += 1;
                piece
            }
            OutputPiece::Unreadable { line, reason } => {
                DisplayPiece::Unreadable { line: *line, reason: reason.clone() }
            }
        };
        spans.push(OutputSpan { note: None, piece });
    }
    spans.entries
}
