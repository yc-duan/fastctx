//! Reversible ownership of the two native tools replaced by the World.

use serde::{Deserialize, Serialize};
use toml_edit::Item;

use crate::source::{Source, comma_in, edit, newline, splice};
use crate::{Error, Notice};

pub(crate) const FEATURES: [&str; 2] = ["shell_tool", "view_image"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    settings: [Option<Setting>; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Setting {
    Replaced { before: Vec<u8> },
    Inserted { prefix: Vec<u8>, suffix: Vec<u8>, inline: bool, comma: bool },
}

pub(crate) fn apply(
    original: &[u8],
    location: &str,
    previous: Option<&Receipt>,
    reinstall: bool,
) -> Result<(Vec<u8>, Receipt, Vec<Notice>), Error> {
    let mut bytes = original.to_vec();
    let mut receipt = previous.cloned().unwrap_or_default();
    let mut notices = Vec::new();
    for (index, key) in FEATURES.iter().enumerate() {
        let source = Source::parse(&bytes, location)?;
        let path = ["features", key];
        if previous.is_some() && !reinstall {
            // What is owned is the value written. A configuration manager
            // that rewrites the whole file keeps the `false` and changes
            // the bytes around it; only the user turning it back on, or
            // removing it, gives it up.
            if source.item(&path).and_then(Item::as_bool) != Some(false) {
                receipt.settings[index] = None;
                notices.push(notice(location, key, "changed since apply; left untouched"));
            }
            continue;
        }
        let value = source
            .item(&path)
            .map(|item| item.as_bool().ok_or_else(|| Error::at(format!("features.{key}"), "expected a boolean")))
            .transpose()?;
        match value {
            Some(false) => {}
            Some(true) => {
                let range = source.value_range(&path)?.expect("the value exists");
                receipt.settings[index] = Some(Setting::Replaced { before: bytes[range.clone()].to_vec() });
                bytes = splice(&bytes, vec![(range, b"false".to_vec())])?;
            }
            None => {
                let (next, setting) = insert(&source, key)?;
                bytes = next;
                receipt.settings[index] = Some(setting);
            }
        }
    }
    Source::parse(&bytes, location)?;
    Ok((bytes, receipt, notices))
}

pub(crate) fn unapply(original: &[u8], location: &str, receipt: &Receipt) -> Result<(Vec<u8>, Vec<Notice>), Error> {
    let mut bytes = original.to_vec();
    let mut notices = Vec::new();
    for (key, setting) in FEATURES.iter().zip(&receipt.settings).rev() {
        let Some(setting) = setting else {
            continue;
        };
        let source = Source::parse(&bytes, location)?;
        let range = match owned_range(&source, key, setting) {
            Ok(range) => range,
            Err(_) => {
                match rewritten(&source, key, setting)? {
                    Some(next) => {
                        bytes = next;
                        Source::parse(&bytes, location)?;
                    }
                    None => notices.push(notice(location, key, "changed since apply; left untouched")),
                }
                continue;
            }
        };
        if matches!(setting, Setting::Inserted { .. }) && edit::joins_lines(&bytes, &range) {
            bytes = rewritten(&source, key, setting)?.expect("owned value was checked");
            continue;
        }
        let mut edits = Vec::new();
        let replacement = match setting {
            Setting::Replaced { before } => before.clone(),
            Setting::Inserted { inline: true, comma: false, .. } => {
                let table = source
                    .item(&["features"])
                    .and_then(Item::as_inline_table)
                    .ok_or_else(|| Error::at("features", "the inserted inline table member changed"))?;
                let next = table
                    .get_values()
                    .into_iter()
                    .filter(|(keys, _)| keys[0].get() != *key)
                    .map(|(keys, _)| source.span(keys[0].span()).map(|span| span.start))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .filter(|start| *start >= range.end)
                    .min();
                if let Some(next) = next
                    && let Some(comma) = comma_in(&bytes, range.end..next)
                {
                    edits.push((comma..comma + 1, Vec::new()));
                }
                Vec::new()
            }
            Setting::Inserted { .. } => Vec::new(),
        };
        edits.push((range, replacement));
        let next = splice(&bytes, edits)?;
        bytes = if Source::parse(&next, location).is_ok() {
            next
        } else {
            // Matching local bytes do not prove that a rewritten inline
            // table still has the original separator arrangement.
            rewritten(&source, key, setting)?.ok_or_else(|| Error::at(*key, "the recorded value changed"))?
        };
    }
    Ok((bytes, notices))
}

fn owned_range(source: &Source<'_>, key: &str, setting: &Setting) -> Result<std::ops::Range<usize>, Error> {
    let range = source
        .value_range(&["features", key])?
        .filter(|range| source.bytes[range.clone()] == *b"false")
        .ok_or_else(|| Error::at(key, "the recorded value changed"))?;
    match setting {
        Setting::Replaced { .. } => Ok(range),
        Setting::Inserted { prefix, suffix, inline, .. } => {
            if *inline != source.item(&["features"]).is_some_and(Item::is_inline_table) {
                return Err(Error::at(key, "the assignment's table layout changed"));
            }
            let start =
                range.start.checked_sub(prefix.len()).ok_or_else(|| Error::at(key, "the assignment changed"))?;
            let end = range.end + suffix.len();
            if source.bytes.get(start..range.start) != Some(prefix.as_slice())
                || source.bytes.get(range.end..end) != Some(suffix.as_slice())
            {
                return Err(Error::at(key, "the inserted assignment changed"));
            }
            Ok(start..end)
        }
    }
}

fn insert(source: &Source<'_>, key: &str) -> Result<(Vec<u8>, Setting), Error> {
    let nl = newline(source.bytes);
    let (at, prefix, suffix, inline, comma) = match source.item(&["features"]) {
        Some(Item::Value(toml_edit::Value::InlineTable(table))) => {
            let last = table
                .get_values()
                .into_iter()
                .map(|(_, value)| source.span(value.span()))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|span| span.end)
                .max();
            let at = last.unwrap_or(source.span(table.span())?.start + 1);
            (at, format!("{}{key} = ", if last.is_some() { ", " } else { "" }), String::new(), true, last.is_some())
        }
        Some(Item::Table(table)) if !table.is_implicit() && !table.is_dotted() => {
            let start = source.span(table.span())?.start;
            let line = source.bytes[start..].iter().position(|b| *b == b'\n');
            let (at, prefix) = match line {
                Some(offset) => (start + offset + 1, format!("{key} = ")),
                None => (source.bytes.len(), format!("{nl}{key} = ")),
            };
            (at, prefix, nl.to_owned(), false, false)
        }
        Some(item) if !item.is_table() => return Err(Error::at("features", "expected a table")),
        _ => (source.root_offset(), format!("features.{key} = "), nl.to_owned(), false, false),
    };
    let inserted = format!("{prefix}false{suffix}").into_bytes();
    let bytes = splice(source.bytes, vec![(at..at, inserted)])?;
    Source::parse(&bytes, "config.toml")?;
    Ok((bytes, Setting::Inserted { prefix: prefix.into_bytes(), suffix: suffix.into_bytes(), inline, comma }))
}

/// Restore a setting still `false` using the current syntax, including inline
/// and dotted tables. A different value is the user's and remains untouched.
fn rewritten(source: &Source<'_>, key: &str, setting: &Setting) -> Result<Option<Vec<u8>>, Error> {
    let path = ["features", key];
    if source.item(&path).and_then(Item::as_bool) != Some(false) {
        return Ok(None);
    }
    match setting {
        Setting::Replaced { before } => {
            source.value_range(&path)?.map(|range| splice(source.bytes, vec![(range, before.clone())])).transpose()
        }
        Setting::Inserted { .. } => edit::remove(source, &path).map(Some),
    }
}

fn notice(location: &str, key: &str, reason: &str) -> Notice {
    Notice { location: format!("{location}: features.{key}"), reason: reason.into() }
}
