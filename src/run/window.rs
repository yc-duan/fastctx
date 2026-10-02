//! Bounded head/tail selection with log byte coordinates, independent of output length.

use super::{ByteRange, CHARACTER_BUDGET, IMAGE_COST};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub(super) enum Body {
    Text(String),
    Image {
        protocol: Protocol,
        complete: bool,
        source: ByteRange,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Protocol {
    Iterm,
    Kitty,
}

impl Protocol {
    pub fn name(self) -> &'static str {
        match self {
            Self::Iterm => "OSC 1337",
            Self::Kitty => "kitty",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Piece {
    pub range: ByteRange,
    pub body: Body,
}

impl Piece {
    pub fn cost(&self) -> usize {
        match &self.body {
            Body::Text(text) => text.chars().count(),
            // Reserve exactly the largest dimensional annotation (two u32 decimal values).
            Body::Image { protocol, .. } => IMAGE_COST + format!("\n=== {} image 4294967295x4294967295 (resized from 4294967295x4294967295) ===\n", protocol.name()).len(),
        }
    }

    fn prefix(&self, budget: usize) -> Option<Self> {
        if self.cost() <= budget {
            return Some(self.clone());
        }
        let Body::Text(text) = &self.body else {
            return None;
        };
        if budget == 0 || self.range.end - self.range.start != text.len() as u64 {
            return None;
        }
        let bytes = if text.is_ascii() {
            budget
        } else {
            text.char_indices()
                .nth(budget)
                .map_or(text.len(), |(offset, _)| offset)
        };
        Some(Self {
            range: ByteRange {
                start: self.range.start,
                end: self.range.start + bytes as u64,
            },
            body: Body::Text(text[..bytes].to_owned()),
        })
    }

    fn suffix(&self, budget: usize) -> Option<Self> {
        if self.cost() <= budget {
            return Some(self.clone());
        }
        let Body::Text(text) = &self.body else {
            return None;
        };
        if budget == 0 || self.range.end - self.range.start != text.len() as u64 {
            return None;
        }
        let offset = if text.is_ascii() {
            text.len() - budget
        } else {
            text.char_indices()
                .rev()
                .nth(budget - 1)
                .map_or(0, |(offset, _)| offset)
        };
        Some(Self {
            range: ByteRange {
                start: self.range.start + offset as u64,
                end: self.range.end,
            },
            body: Body::Text(text[offset..].to_owned()),
        })
    }
}

#[derive(Default)]
pub(super) struct Window {
    head: Vec<Piece>,
    tail: VecDeque<Piece>,
    head_cost: usize,
    tail_cost: usize,
    pub total_cost: u64,
    last_end: u64,
}

impl Window {
    pub fn push(&mut self, mut piece: Piece) {
        // Chunked images appear at their last chunk; retain their full source separately
        // from the presentation boundary after any text emitted between chunks.
        piece.range.start = piece.range.start.max(self.last_end);
        self.last_end = piece.range.end;
        let cost = piece.cost();
        self.total_cost += cost as u64;
        if self.head_cost < CHARACTER_BUDGET {
            if let Some(prefix) = piece.prefix(CHARACTER_BUDGET - self.head_cost) {
                self.head_cost += prefix.cost();
                self.head.push(prefix);
            }
            // A prefix cannot skip a boundary image and resume at later text.
            if self.total_cost > CHARACTER_BUDGET as u64 {
                self.head_cost = CHARACTER_BUDGET;
            }
        }
        self.tail_cost += cost;
        self.tail.push_back(piece);
        while self.tail_cost > CHARACTER_BUDGET {
            let first = self.tail.pop_front().expect("nonempty over-budget tail");
            let size = first.cost();
            let excess = self.tail_cost - CHARACTER_BUDGET;
            self.tail_cost -= size;
            if excess < size
                && let Some(suffix) = first.suffix(size - excess)
            {
                self.tail_cost += suffix.cost();
                self.tail.push_front(suffix);
            }
        }
    }

    pub fn select(self, budget: usize, omission_cost: usize) -> (Vec<Piece>, Option<ByteRange>) {
        if self.total_cost <= budget as u64 {
            return (self.head, None);
        }
        let usable = budget.saturating_sub(omission_cost);
        let mut head = Vec::new();
        let mut used = 0;
        for piece in self.head {
            let Some(prefix) = piece.prefix(usable / 2 - used) else {
                break;
            };
            used += prefix.cost();
            let partial = prefix.range.end != piece.range.end;
            head.push(prefix);
            if partial {
                break;
            }
        }
        let mut tail = Vec::new();
        let mut tail_used = 0;
        for piece in self.tail.into_iter().rev() {
            let Some(suffix) = piece.suffix(usable - used - tail_used) else {
                break;
            };
            tail_used += suffix.cost();
            let partial = suffix.range.start != piece.range.start;
            tail.push(suffix);
            if partial {
                break;
            }
        }
        tail.reverse();
        let omitted = ByteRange {
            start: head.last().map_or(0, |piece| piece.range.end),
            end: tail
                .first()
                .map_or(self.last_end, |piece| piece.range.start),
        };
        head.extend(tail);
        (head, Some(omitted))
    }
}
