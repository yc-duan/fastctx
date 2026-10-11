//! Counting and shortening the complete content of a tool answer.

use crate::run::Content;
use crate::run::window::{IMAGE_COST, bytes, units};

#[derive(Clone, Debug)]
pub struct Kept {
    pub source: usize,
    pub output: usize,
    pub bytes: Option<std::ops::Range<usize>>,
}

pub fn fit_mapped(content: &[Content], budget: usize, omission: &str) -> (Vec<Content>, Vec<Kept>) {
    if cost(content) <= budget {
        return (
            content.to_vec(),
            content
                .iter()
                .enumerate()
                .map(|(index, block)| Kept {
                    source: index,
                    output: index,
                    bytes: match block {
                        Content::Text(text) => Some(0..text.len()),
                        _ => None,
                    },
                })
                .collect(),
        );
    }
    let available = budget.checked_sub(units(omission)).expect("room for the source notice");
    let header = first_line_cost(content);
    let head = if header <= available { (available / 4).max(header) } else { available / 4 };
    let (mut start, mut regions) = mapped_edge(content, head, false);
    let (end, mut last) = mapped_edge(content, available - cost(&start), true);
    start.push(Content::Text(omission.to_string()));
    for region in &mut last {
        region.output += start.len();
    }
    start.extend(end);
    regions.extend(last);
    (start, regions)
}

fn mapped_edge(content: &[Content], mut budget: usize, back: bool) -> (Vec<Content>, Vec<Kept>) {
    let mut kept = Vec::new();
    let mut regions = Vec::new();
    let blocks: Box<dyn Iterator<Item = (usize, &Content)>> =
        if back { Box::new(content.iter().enumerate().rev()) } else { Box::new(content.iter().enumerate()) };
    for (index, block) in blocks {
        let size = block_cost(block);
        let bytes = match block {
            Content::Text(text) if size > budget => {
                let selected = if back { suffix(text, bytes(budget)) } else { prefix(text, bytes(budget)) };
                if selected.is_empty() {
                    break;
                }
                let from = if back { text.len() - selected.len() } else { 0 };
                kept.push(Content::Text(selected.to_string()));
                regions.push(Kept { source: index, output: 0, bytes: Some(from..from + selected.len()) });
                break;
            }
            Content::Text(text) => Some(0..text.len()),
            Content::Image { .. } if size > budget => break,
            Content::Image { .. } => None,
        };
        kept.push(block.clone());
        regions.push(Kept { source: index, output: 0, bytes });
        budget -= size;
    }
    if back {
        kept.reverse();
        regions.reverse();
    }
    for (index, region) in regions.iter_mut().enumerate() {
        region.output = index;
    }
    (kept, regions)
}

/// What the content costs, in the units Codex counts: each text block its
/// UTF-8 bytes divided by four, rounded up, and each image its fixed cost.
pub fn cost(content: &[Content]) -> usize {
    content.iter().map(block_cost).sum()
}

fn block_cost(block: &Content) -> usize {
    match block {
        Content::Text(text) => units(text),
        Content::Image { .. } => IMAGE_COST,
    }
}

/// Keeps the start and end, paying for the omission notice before either.
/// Images are indivisible. The notice must identify a durable full source.
pub fn fit(content: Vec<Content>, budget: usize, omission: &str) -> Vec<Content> {
    if cost(&content) <= budget {
        return content;
    }
    let available = budget.checked_sub(units(omission)).expect("room for the source notice");
    let header = first_line_cost(&content);
    let head = if header <= available { (available / 4).max(header) } else { available / 4 };
    let mut start = edge(&content, head, false);
    let end = edge(&content, available - cost(&start), true);
    start.push(Content::Text(omission.to_string()));
    start.extend(end);
    start
}

/// The first complete line, normally the tool's status and machine.
pub fn first_line_cost(content: &[Content]) -> usize {
    match content.first() {
        Some(Content::Text(text)) => text.split_once('\n').map_or(0, |(line, _)| (line.len() + 1).div_ceil(4)),
        _ => 0,
    }
}

fn edge(content: &[Content], mut budget: usize, back: bool) -> Vec<Content> {
    let mut kept = Vec::new();
    let blocks: Box<dyn Iterator<Item = &Content>> =
        if back { Box::new(content.iter().rev()) } else { Box::new(content.iter()) };
    for block in blocks {
        let size = block_cost(block);
        if size <= budget {
            kept.push(block.clone());
            budget -= size;
        } else {
            if let Content::Text(text) = block {
                let text = if back { suffix(text, bytes(budget)) } else { prefix(text, bytes(budget)) };
                if !text.is_empty() {
                    kept.push(Content::Text(text.to_string()));
                }
            }
            break;
        }
    }
    if back {
        kept.reverse();
    }
    kept
}

/// The longest start of `text` within `room` bytes, whole characters only.
pub fn prefix(text: &str, room: usize) -> &str {
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The longest end of `text` within `room` bytes, whole characters only.
pub fn suffix(text: &str, room: usize) -> &str {
    let mut start = text.len().saturating_sub(room);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
