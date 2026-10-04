//! Counting and shortening the complete content of a tool answer.

use crate::run::Content;
use crate::run::window::IMAGE_COST;

/// Unicode scalar values in text, with each image charged at its fixed cost.
pub fn cost(content: &[Content]) -> usize {
    content.iter().map(block_cost).sum()
}

fn block_cost(block: &Content) -> usize {
    match block {
        Content::Text(text) => text.chars().count(),
        Content::Image { .. } => IMAGE_COST,
    }
}

/// Keeps the start and end, paying for the omission notice before either.
/// Images are indivisible. The notice must identify a durable full source.
pub fn fit(content: Vec<Content>, budget: usize, omission: &str) -> Vec<Content> {
    if cost(&content) <= budget {
        return content;
    }
    let available = budget.checked_sub(omission.chars().count()).expect("room for the source notice");
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
        Some(Content::Text(text)) => text.split_once('\n').map_or(0, |(line, _)| line.chars().count() + 1),
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
                let text =
                    if back { text.chars().skip(size - budget).collect() } else { text.chars().take(budget).collect() };
                if budget > 0 {
                    kept.push(Content::Text(text));
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
