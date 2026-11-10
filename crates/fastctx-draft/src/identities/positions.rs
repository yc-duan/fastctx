//! Resolve only requested clock ranges; do not materialize the document's
//! per-scalar identities. Adjacent deletions become one Yrs removal.

use super::Id;
use crate::{Document, Result, error};
use std::collections::{BTreeSet, HashMap};
use yrs::Transact;

#[derive(Clone, Copy)]
struct Position {
    at: u32,
    length: u32,
}
pub(super) struct Resolved {
    pub anchor: u32,
    pub ranges: Vec<(u32, u32)>,
    pub removed: usize,
}

pub(super) fn resolve(document: &Document, after: Option<Id>, delete: &[Id]) -> Result<Resolved> {
    if after.is_none() && delete.is_empty() {
        return Ok(Resolved { anchor: 0, ranges: Vec::new(), removed: 0 });
    }
    let mut needed: HashMap<u64, BTreeSet<u64>> = HashMap::new();
    for id in after.iter().chain(delete) {
        if id.client >= 1 << 53 {
            return Err(error("the draft character has an invalid client id"));
        }
        needed.entry(id.client).or_default().insert(id.clock.into());
    }
    let mut positions = HashMap::with_capacity(delete.len() + usize::from(after.is_some()));
    let transaction = document.doc.transact();
    let mut at = 0;
    document.text.visit_chunks(&transaction, |chunk| {
        let visible = !chunk.deleted && chunk.countable;
        if let Some(clocks) = needed.get(&chunk.id.client.get()) {
            let start = u64::from(chunk.id.clock);
            let mut requested = clocks.range(start..start + u64::from(chunk.len)).peekable();
            if requested.peek().is_some() {
                let mut save = |clock: u64, offset, length| {
                    positions.insert(
                        Id { client: chunk.id.client.get(), clock: clock as u32 },
                        Position { at: at + offset, length },
                    );
                };
                if !visible {
                    for &clock in requested {
                        save(clock, 0, 0);
                    }
                } else if let Some(value) = chunk.text {
                    if value.is_ascii() {
                        for &clock in requested {
                            save(clock, (clock - start) as u32, 1);
                        }
                    } else {
                        let mut offset = 0;
                        for character in value.chars() {
                            let clock = start + u64::from(offset);
                            while requested.peek().is_some_and(|&&wanted| wanted < clock) {
                                requested.next();
                            }
                            if requested.peek().is_some_and(|&&wanted| wanted == clock) {
                                save(clock, offset, character.len_utf16() as u32);
                                requested.next();
                            }
                            if requested.peek().is_none() {
                                break;
                            }
                            offset += character.len_utf16() as u32;
                        }
                    }
                } else {
                    for &clock in requested {
                        save(clock, (clock - start) as u32, 0);
                    }
                }
            }
        }
        if visible {
            at += chunk.len;
        }
    });
    let anchor = match after {
        Some(id) => {
            let position = positions
                .get(&id)
                .ok_or_else(|| error("the insertion anchor is not in this draft or splits a UTF-16 character"))?;
            position.at + position.length
        }
        None => 0,
    };
    let mut ranges = Vec::with_capacity(delete.len());
    for id in delete {
        let position =
            positions.get(id).ok_or_else(|| error("a character is not in this draft or splits a UTF-16 character"))?;
        if position.length != 0 {
            ranges.push((position.at, position.length));
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    let removed = ranges.len();
    let before = ranges.iter().map(|&(start, length)| anchor.saturating_sub(start).min(length)).sum::<u32>();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, length) in ranges {
        if let Some(last) = merged.last_mut()
            && last.0 + last.1 == start
        {
            last.1 += length;
        } else {
            merged.push((start, length));
        }
    }
    Ok(Resolved { anchor: anchor - before, ranges: merged, removed })
}
