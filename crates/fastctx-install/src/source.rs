//! TOML supplies syntax locations; edits splice the original bytes.

use std::ops::Range;

use toml_edit::{Document, Item, Table};

use crate::Error;

pub(crate) mod edit;

/// Compare TOML values without attaching ownership to their source spelling.
pub(crate) fn same_value(left: &Item, right: &Item) -> bool {
    if let (Some(left), Some(right)) = (left.as_table_like(), right.as_table_like()) {
        return left.len() == right.len()
            && left.iter().all(|(key, value)| right.get(key).is_some_and(|other| same_value(value, other)));
    }
    match (left, right) {
        (Item::Value(left), Item::Value(right)) => same_literal(left, right),
        (Item::ArrayOfTables(left), Item::ArrayOfTables(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| same_value(&Item::Table(left.clone()), &Item::Table(right.clone())))
        }
        _ => false,
    }
}

fn same_literal(left: &toml_edit::Value, right: &toml_edit::Value) -> bool {
    use toml_edit::Value;
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.value() == right.value(),
        (Value::Integer(left), Value::Integer(right)) => left.value() == right.value(),
        (Value::Float(left), Value::Float(right)) => {
            left.value() == right.value() || (left.value().is_nan() && right.value().is_nan())
        }
        (Value::Boolean(left), Value::Boolean(right)) => left.value() == right.value(),
        (Value::Datetime(left), Value::Datetime(right)) => left.value() == right.value(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right.iter()).all(|(left, right)| same_literal(left, right))
        }
        (Value::InlineTable(left), Value::InlineTable(right)) => {
            same_value(&Item::Value(left.clone().into()), &Item::Value(right.clone().into()))
        }
        _ => false,
    }
}

pub(crate) struct Source<'a> {
    pub bytes: &'a [u8],
    pub doc: Document<&'a str>,
    offset: usize,
    location: &'a str,
}

impl<'a> Source<'a> {
    pub fn parse(bytes: &'a [u8], location: &'a str) -> Result<Self, Error> {
        let offset = usize::from(bytes.starts_with(b"\xef\xbb\xbf")) * 3;
        let text =
            std::str::from_utf8(&bytes[offset..]).map_err(|_| Error::at(location, "config.toml is not UTF-8"))?;
        let doc = Document::parse(text).map_err(|error| {
            // TOML diagnostics quote source lines, which may contain unrelated secrets.
            Error::at(location, format!("invalid TOML at {:?}: {}", error.span(), error.message()))
        })?;
        Ok(Self { bytes, doc, offset, location })
    }

    pub fn item(&self, path: &[&str]) -> Option<&Item> {
        let mut item = self.doc.as_item();
        for key in path {
            item = item.as_table_like()?.get(key)?;
        }
        Some(item)
    }

    pub fn span(&self, range: Option<Range<usize>>) -> Result<Range<usize>, Error> {
        range
            .map(|r| r.start + self.offset..r.end + self.offset)
            .ok_or_else(|| Error::at(self.location, "TOML source range is unavailable"))
    }

    pub fn value_range(&self, path: &[&str]) -> Result<Option<Range<usize>>, Error> {
        self.item(path).map(|i| self.span(i.span())).transpose()
    }

    pub fn root_offset(&self) -> usize {
        self.offset
    }

    /// A normal table can consist of disjoint sections interleaved with the
    /// user's tables. Never remove the gap between two of its sections.
    pub fn table_ranges(&self, path: &[&str]) -> Result<Vec<Range<usize>>, Error> {
        let Some(item) = self.item(path) else { return Ok(vec![]) };
        let table = item.as_table().ok_or_else(|| Error::at(path.join("."), "expected a section table"))?;
        let mut ranges = vec![];
        self.collect(table, &mut ranges)?;
        ranges.sort_by_key(|r| r.start);
        if ranges.windows(2).any(|p| p[0].end > p[1].start) {
            return Err(Error::at(self.location, "overlapping TOML sections"));
        }
        Ok(ranges)
    }

    fn collect(&self, table: &Table, ranges: &mut Vec<Range<usize>>) -> Result<(), Error> {
        if !table.is_implicit() && !table.is_dotted() {
            let range = self.span(table.span())?;
            ranges.push(range);
        }
        for (_, child) in table.iter() {
            if let Some(table) = child.as_table()
                && !table.is_dotted()
            {
                self.collect(table, ranges)?;
            }
        }
        for (keys, value) in table.get_values() {
            let start = self.span(keys[0].span())?.start;
            let end = self.span(value.span())?.end;
            ranges.push(start..end);
        }
        Ok(())
    }

    pub fn assignment(&self, path: &[&str]) -> Result<Range<usize>, Error> {
        let parent = self
            .item(&path[..path.len() - 1])
            .and_then(Item::as_table_like)
            .ok_or_else(|| Error::at(path.join("."), "missing parent table"))?;
        let (key, value) =
            parent.get_key_value(path[path.len() - 1]).ok_or_else(|| Error::at(path.join("."), "missing key"))?;
        let start = self.span(key.span())?.start;
        let end = self.span(value.span())?.end;
        Ok(start..end)
    }

    /// Parent keys in dotted assignments share parser nodes and their spans
    /// can refer to a different statement. The leaf key has its own location.
    pub fn statement(&self, keys: &[&toml_edit::Key], value: &toml_edit::Value) -> Result<Range<usize>, Error> {
        let mut start = self.span(keys.last().expect("an assignment has a key").span())?.start;
        for _ in 1..keys.len() {
            while start > self.offset && matches!(self.bytes[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            if start == self.offset || self.bytes[start - 1] != b'.' {
                return Err(Error::at(self.location, "dotted key separator is missing"));
            }
            start -= 1;
            while start > self.offset && matches!(self.bytes[start - 1], b' ' | b'\t') {
                start -= 1;
            }
            let last = start.checked_sub(1).ok_or_else(|| Error::at(self.location, "missing dotted key"))?;
            let quote = self.bytes[last];
            if matches!(quote, b'\'' | b'"') {
                start = last;
                loop {
                    start = start.checked_sub(1).ok_or_else(|| Error::at(self.location, "missing key quote"))?;
                    if self.bytes[start] == quote {
                        let escapes = self.bytes[..start].iter().rev().take_while(|byte| **byte == b'\\').count();
                        if quote == b'\'' || escapes % 2 == 0 {
                            break;
                        }
                    }
                }
            } else {
                start = last;
                while start > self.offset
                    && (self.bytes[start - 1].is_ascii_alphanumeric() || matches!(self.bytes[start - 1], b'_' | b'-'))
                {
                    start -= 1;
                }
            }
        }
        Ok(start..self.span(value.span())?.end)
    }
}

pub(crate) fn splice(bytes: &[u8], mut edits: Vec<(Range<usize>, Vec<u8>)>) -> Result<Vec<u8>, Error> {
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = vec![];
    let mut cursor = 0;
    for (range, replacement) in edits {
        if range.start < cursor || range.end < range.start || range.end > bytes.len() {
            return Err(Error::at("receipt", "overlapping or out-of-bounds byte edits"));
        }
        result.extend_from_slice(&bytes[cursor..range.start]);
        result.extend_from_slice(&replacement);
        cursor = range.end;
    }
    result.extend_from_slice(&bytes[cursor..]);
    Ok(result)
}

pub(crate) fn newline(bytes: &[u8]) -> &'static str {
    match bytes.iter().position(|b| *b == b'\n') {
        Some(i) if i > 0 && bytes[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

pub(crate) fn comma_in(bytes: &[u8], range: Range<usize>) -> Option<usize> {
    let mut comment = false;
    for i in range {
        match bytes[i] {
            b'\n' => comment = false,
            b'#' => comment = true,
            b',' if !comment => return Some(i),
            _ => {}
        }
    }
    None
}
