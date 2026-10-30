//! Retain both edges that any final answer budget can reach.
//!
//! Serve always applies answer::fit with at most BUDGET units. After either
//! retained edge alone costs at least BUDGET, the interior cannot be reached
//! by that operation, including when waiting messages reduce the budget.
//! No marker is inserted here: the existing final fit owns its exact notice.

use super::window::{BUDGET, IMAGE_COST, bytes};
use fastctx_types::run::OutputPiece;
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Pieces {
    head: Vec<OutputPiece>,
    head_cost: usize,
    tail: VecDeque<OutputPiece>,
    tail_cost: usize,
    partial: Option<OutputPiece>,
    next_line: Option<u64>,
}

impl Pieces {
    pub fn push(&mut self, piece: OutputPiece) {
        if let OutputPiece::Text { first_line, text } = piece {
            self.text(first_line, &text);
        } else {
            self.push_raw(piece);
        }
    }
    fn push_raw(&mut self, piece: OutputPiece) {
        self.next_line = None;
        let cost = lower_cost(&piece);
        if self.head_cost < BUDGET {
            self.head_cost += cost;
            self.head.push(piece);
        } else {
            self.tail_cost += cost;
            self.tail.push_back(piece);
            self.trim();
        }
    }
    pub fn text(&mut self, mut line: u64, text: &str) {
        let mut text = text;
        while !text.is_empty() {
            let mut end = text.len().min(bytes(BUDGET));
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text_part(line, &text[..end]);
            line += memchr::memchr_iter(b'\n', &text.as_bytes()[..end]).count() as u64;
            text = &text[end..];
        }
    }
    fn text_part(&mut self, line: u64, text: &str) {
        if self.head_cost < BUDGET {
            if let Some(OutputPiece::Text { text: open, .. }) = self.head.last_mut()
                && self.next_line == Some(line)
                && open.len() + text.len() <= bytes(BUDGET)
            {
                let before = open.len() / 4;
                open.push_str(text);
                self.head_cost += open.len() / 4 - before;
            } else {
                self.push_raw(OutputPiece::Text { first_line: line, text: text.to_string() });
            }
        } else if let Some(OutputPiece::Text { text: open, .. }) = self.tail.back_mut()
            && self.next_line == Some(line)
            && open.len() + text.len() <= bytes(BUDGET)
        {
            let before = open.len() / 4;
            open.push_str(text);
            self.tail_cost += open.len() / 4 - before;
            self.trim();
        } else {
            self.push_raw(OutputPiece::Text { first_line: line, text: text.to_string() });
        }
        self.next_line = Some(line + memchr::memchr_iter(b'\n', text.as_bytes()).count() as u64);
    }
    fn trim(&mut self) {
        while let Some(first) = self.tail.front() {
            let cost = lower_cost(first);
            if self.tail_cost - cost < BUDGET {
                break;
            }
            let discarded = self.tail.pop_front().expect("front");
            if self.partial.is_none()
                && matches!(discarded, OutputPiece::Omitted { .. } | OutputPiece::PartOfLine { .. })
            {
                // Present uses this fact for the log footer, even when the
                // original marker is beyond either final answer edge.
                self.partial = Some(discarded);
            }
            self.tail_cost -= cost;
        }
    }
    pub fn finish(mut self) -> Vec<OutputPiece> {
        self.head.extend(self.partial);
        self.head.extend(self.tail);
        self.head
    }
}

/// Floors make text-block coalescing harmless. A rendered diagnostic or range
/// note adds more than four bytes, including its newline, beyond the text
/// counted here. Image captions can only increase the lower bound.
fn lower_cost(piece: &OutputPiece) -> usize {
    match piece {
        OutputPiece::Text { text, .. } => text.len() / 4,
        OutputPiece::PartOfLine { text, .. } => text.len() / 4 + 1,
        OutputPiece::Image { .. } => IMAGE_COST,
        OutputPiece::Unreadable { reason, .. } => reason.len() / 4 + 1,
        OutputPiece::Omitted { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answer::{cost, fit};
    use crate::run::present::{Content, present};
    use fastctx_types::run::{ExitStatus, OutputWindow, RunReply, RunState};

    fn render(pieces: Vec<OutputPiece>) -> Vec<Content> {
        present(&RunReply {
            scratch: Vec::new(),
            run: None,
            consumed: None,
            privacy: Vec::new(),
            missing: Vec::new(),
            machine: "probe".try_into().unwrap(),
            job: "j-aaaaaa".try_into().unwrap(),
            state: RunState::Ended { status: ExitStatus::Code(0) },
            output: OutputWindow {
                log: "C:/source/output.log".into(),
                bytes: 1,
                lines: 10_000,
                pieces,
                legacy: Vec::new(),
                escaped_lines: 0,
                failed: Vec::new(),
            },
            visited: Vec::new(),
            answered: None,
        })
    }

    #[test]
    fn final_budget_and_caption_contract_survives_early_release() {
        let note = "\n=== the answer stops here, at its limit; all of the output: probe C:/source/output.log ===\n";
        let mut mixed = Vec::new();
        let mut diagnostics = Vec::new();
        for index in 0..super::super::window::OUTPUT_WINDOW / 5 {
            mixed.push(OutputPiece::Text {
                first_line: index as u64 + 1,
                text: format!("=== picture {index} (image 1x2, image/gif) ===\n"),
            });
            mixed.push(OutputPiece::Image {
                mime: "image/gif".into(),
                width: 1,
                height: 2,
                base64: format!("opaque-image-{index}"),
            });
            mixed.push(OutputPiece::Omitted { first_line: index as u64 + 1, last_line: index as u64 + 2 });
            diagnostics.push(OutputPiece::Unreadable {
                line: index as u64 + 1,
                reason: format!("graphic {index}: {}", "x".repeat(200)),
            });
        }
        let unicode =
            vec![OutputPiece::Text { first_line: 1, text: format!("begin\n{}end\n", "😀中\n".repeat(2 * BUDGET)) }];
        let mut gap = Vec::new();
        for index in 0..81 {
            gap.push(if index == 40 {
                OutputPiece::Omitted { first_line: 41, last_line: 99 }
            } else {
                OutputPiece::Image { mime: "image/gif".into(), width: 1, height: 2, base64: format!("image-{index}") }
            });
        }
        for original in [mixed, diagnostics, unicode, gap] {
            let mut kept = Pieces::default();
            for piece in original.iter().cloned() {
                kept.push(piece);
            }
            let before = render(original);
            let after = render(kept.finish());
            assert!(cost(&before) > BUDGET && cost(&after) > BUDGET);
            for budget in [128, IMAGE_COST - 1, IMAGE_COST, IMAGE_COST + 1, BUDGET / 2, BUDGET - 1, BUDGET] {
                assert_eq!(fit(before.clone(), budget, note), fit(after.clone(), budget, note));
            }
        }
    }
}
