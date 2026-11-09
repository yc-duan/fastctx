//! The registered byte-tree form uses canonical decimal interval keys.
use super::{Address, Child, Span};
use crate::{
    manifest::{ContentBlock, ContentRef, ManifestChild},
    store::sql,
};
use fastctx_storage::Result;

pub(super) fn blocks(address: &Address, offset: u64, blocks: Vec<ContentBlock>) -> Result<(Span, Vec<Child>)> {
    let mut through = offset;
    let mut children = Vec::with_capacity(blocks.len());
    for block in blocks {
        let start = through;
        through = through
            .checked_add(u64::from(block.decoded_bytes))
            .ok_or_else(|| sql::invalid("supply block interval overflows"))?;
        children.push(Child { address: address.bytes(Span::new(start, through)?)?, root: block.into() });
    }
    Ok((Span::new(offset, through)?, children))
}

pub(super) fn intervals(children: &[ManifestChild]) -> Option<Vec<Span>> {
    if children.is_empty() {
        return None;
    }
    let mut previous = None;
    let mut ranges = Vec::with_capacity(children.len());
    for child in children {
        let first = decimal(&child.first_key)?;
        let last = decimal(&child.last_key)?;
        let end = last.checked_add(1)?;
        if last < first || child.entries.0 == 0 || previous.is_some_and(|end| end != first) {
            return None;
        }
        ranges.push(Span { start: first, end });
        previous = Some(end);
    }
    Some(ranges)
}

fn decimal(key: &str) -> Option<u64> {
    let value = key.parse::<u64>().ok()?;
    (format!("{value:020}") == key).then_some(value)
}

pub(super) fn branch(address: &Address, children: Vec<ManifestChild>, ranges: Vec<Span>) -> Result<(Span, Vec<Child>)> {
    let first = ranges.first().ok_or_else(|| sql::invalid("empty supply byte branch"))?;
    let last = ranges.last().ok_or_else(|| sql::invalid("empty supply byte branch"))?;
    let extent = Span::new(first.start, last.end)?;
    let children = children
        .into_iter()
        .zip(ranges)
        .map(|(child, span)| {
            Ok(Child { address: address.bytes(span)?, root: ContentRef::Manifest { root: child.root } })
        })
        .collect::<Result<_>>()?;
    Ok((extent, children))
}
