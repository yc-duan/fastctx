//! Splice owned TOML syntax using today's layout, preserving adjacent user text.

use std::ops::Range;

use toml_edit::{Item, Table, Value};

use super::{Source, comma_in, newline, same_value, splice};
use crate::Error;

/// An inserted newline can become necessary when the user appends another
/// statement. Removing it would join that statement to the original last line.
pub(crate) fn joins_lines(bytes: &[u8], range: &Range<usize>) -> bool {
    range.start > 0
        && range.end < bytes.len()
        && bytes[range.start - 1] != b'\n'
        && !matches!(bytes[range.end], b'\r' | b'\n')
        && bytes[range.clone()].contains(&b'\n')
}

pub(crate) fn remove(source: &Source<'_>, path: &[&str]) -> Result<Vec<u8>, Error> {
    let mut ranges = Vec::new();
    collect_table(source, source.doc.as_table(), &[], path, &mut ranges)?;
    ranges.sort_by_key(|range| range.start);
    let edits = ranges
        .into_iter()
        .map(|range| {
            let comments = comments(&source.bytes[range.clone()]);
            (range, comments)
        })
        .collect();
    let next = splice(source.bytes, edits)?;
    if Source::parse(&next, source.location)?.item(path).is_some() {
        return Err(Error::at(path.join("."), "owned TOML syntax remains after removal"));
    }
    Ok(next)
}

/// Comments can move inside an owned array or inline table when a manager
/// rewrites it. Keep them even when the surrounding owned value is removed.
fn comments(bytes: &[u8]) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'#' => {
                let mut start = at;
                while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
                    start -= 1;
                }
                let end = bytes[at..].iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |end| at + end + 1);
                kept.extend_from_slice(&bytes[start..end]);
                at = end;
            }
            quote @ (b'\'' | b'"') => {
                let triple = bytes.get(at..at + 3).is_some_and(|value| value == [quote; 3]);
                at += if triple { 3 } else { 1 };
                while at < bytes.len() {
                    if bytes[at] == b'\\' && quote == b'"' {
                        at += 2;
                    } else if bytes[at] == quote {
                        let count = bytes[at..].iter().take_while(|byte| **byte == quote).count();
                        at += if triple { count } else { 1 };
                        if !triple || count >= 3 {
                            break;
                        }
                    } else {
                        at += 1;
                    }
                }
            }
            _ => at += 1,
        }
    }
    kept
}

fn collect_table(
    source: &Source<'_>,
    table: &Table,
    path: &[&str],
    target: &[&str],
    ranges: &mut Vec<Range<usize>>,
) -> Result<(), Error> {
    if path.starts_with(target) && !table.is_implicit() && !table.is_dotted() {
        ranges.push(line(source.bytes, source.span(table.span())?));
    }
    if !table.is_dotted() {
        collect_values(source, table, path, target, None, ranges)?;
    }
    for (key, item) in table.iter() {
        if let Some(child) = item.as_table() {
            let mut path = path.to_vec();
            path.push(key);
            collect_table(source, child, &path, target, ranges)?;
        }
    }
    Ok(())
}

fn collect_values(
    source: &Source<'_>,
    table: &dyn toml_edit::TableLike,
    path: &[&str],
    target: &[&str],
    inline_end: Option<usize>,
    ranges: &mut Vec<Range<usize>>,
) -> Result<(), Error> {
    let mut members = table
        .get_values()
        .into_iter()
        .map(|(keys, value)| {
            let range = source.statement(&keys, value)?;
            let mut path = path.to_vec();
            path.extend(keys.iter().map(|key| key.get()));
            Ok((range, path, value))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    members.sort_by_key(|(range, _, _)| range.start);
    let selected: Vec<_> = members.iter().map(|(_, path, _)| path.starts_with(target)).collect();
    for (index, (range, path, value)) in members.iter().enumerate() {
        if selected[index] {
            ranges.push(if inline_end.is_some() { range.clone() } else { line(source.bytes, range.clone()) });
        } else if target.starts_with(path)
            && let Some(child) = value.as_inline_table()
        {
            collect_values(source, child, path, target, Some(source.span(child.span())?.end - 1), ranges)?;
        }
    }
    if let Some(inline_end) = inline_end {
        // Delete separators for each consecutive removed group. Trivia around
        // the separators belongs to the current file, not to the receipt.
        let mut index = 0;
        while index < members.len() {
            if !selected[index] {
                index += 1;
                continue;
            }
            let start = index;
            while index + 1 < members.len() && selected[index + 1] {
                index += 1;
            }
            let end = index;
            let separators = if end + 1 < members.len() { start..end + 1 } else { start.saturating_sub(1)..end };
            for left in separators {
                let between = members[left].0.end..members[left + 1].0.start;
                let comma = comma_in(source.bytes, between)
                    .ok_or_else(|| Error::at(target.join("."), "inline table separator is missing"))?;
                ranges.push(comma..comma + 1);
            }
            index += 1;
        }
        if selected.last() == Some(&true)
            && let Some(comma) = comma_in(source.bytes, members.last().expect("selected member").0.end..inline_end)
        {
            ranges.push(comma..comma + 1);
        }
    }
    Ok(())
}

/// Remove the whole line only when it contains nothing besides this syntax.
/// Comments, including comments after a removed header, remain byte for byte.
fn line(bytes: &[u8], range: Range<usize>) -> Range<usize> {
    let start = bytes[..range.start].iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    let start = if start == 0 && bytes.starts_with(b"\xef\xbb\xbf") { 3 } else { start };
    let end = bytes[range.end..].iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |at| range.end + at + 1);
    if bytes[start..range.start].iter().all(|byte| matches!(byte, b' ' | b'\t'))
        && bytes[range.end..end].iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        start..end
    } else {
        range
    }
}

/// Update an already owned subtree without reserializing the user's document.
pub(crate) fn synchronize(bytes: &[u8], location: &str, path: &[&str], desired: &Item) -> Result<Vec<u8>, Error> {
    let source = Source::parse(bytes, location)?;
    let current = source.item(path);
    if current.is_some_and(|current| same_value(current, desired)) {
        return Ok(bytes.to_vec());
    }
    if let Some(table) = desired.as_table_like() {
        let mut next = bytes.to_vec();
        if let Some(current) = current.and_then(Item::as_table_like) {
            for (key, _) in current.iter().filter(|(key, _)| !table.contains_key(key)) {
                let mut child = path.to_vec();
                child.push(key);
                next = remove(&Source::parse(&next, location)?, &child)?;
            }
        }
        for (key, value) in table.iter() {
            let mut child = path.to_vec();
            child.push(key);
            next = synchronize(&next, location, &child, value)?;
        }
        return Ok(next);
    }
    let mut replacement = desired.to_string().trim().as_bytes().to_vec();
    let next = if let Some(current) = current {
        let range = source.span(current.span())?;
        if desired.is_array() {
            let kept = comments(&bytes[range.clone()]);
            if !kept.is_empty() {
                let mut prefix = newline(bytes).as_bytes().to_vec();
                prefix.extend_from_slice(&kept);
                replacement.splice(1..1, prefix);
            }
        }
        splice(bytes, vec![(range, replacement)])?
    } else {
        insert(&source, path, &replacement)?
    };
    Source::parse(&next, location)?;
    Ok(next)
}

fn insert(source: &Source<'_>, path: &[&str], value: &[u8]) -> Result<Vec<u8>, Error> {
    for length in (0..path.len()).rev() {
        let Some(parent) = source.item(&path[..length]) else { continue };
        let keys = path[length..].iter().map(|key| toml_edit::Key::new(*key).to_string()).collect::<Vec<_>>().join(".");
        let (at, prefix, suffix) = match parent {
            Item::Value(Value::InlineTable(table)) => {
                let last = table
                    .get_values()
                    .into_iter()
                    .map(|(_, value)| source.span(value.span()).map(|range| range.end))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max();
                (last.unwrap_or(source.span(table.span())?.start + 1), if last.is_some() { ", " } else { "" }, "")
            }
            Item::Table(_) if length == 0 => (source.root_offset(), "", newline(source.bytes)),
            Item::Table(table) if !table.is_implicit() && !table.is_dotted() => {
                let start = source.span(table.span())?.start;
                match source.bytes[start..].iter().position(|byte| *byte == b'\n') {
                    Some(offset) => (start + offset + 1, "", newline(source.bytes)),
                    None => (source.bytes.len(), newline(source.bytes), newline(source.bytes)),
                }
            }
            Item::Table(_) => continue,
            _ => return Err(Error::at(path[..length].join("."), "expected a table")),
        };
        let mut inserted = format!("{prefix}{keys} = ").into_bytes();
        inserted.extend_from_slice(value);
        inserted.extend_from_slice(suffix.as_bytes());
        return splice(source.bytes, vec![(at..at, inserted)]);
    }
    Err(Error::at(path.join("."), "missing root table"))
}
