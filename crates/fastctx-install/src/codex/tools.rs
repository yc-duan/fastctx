//! Reversible ownership of native capabilities replaced by the World.

use serde::{Deserialize, Serialize};
use toml_edit::Item;

use crate::source::{Source, comma_in, edit, newline, splice};
use crate::{Error, Notice};

pub(crate) const FEATURES: [&str; 3] = ["shell_tool", "view_image", "goals"];
const SETTINGS: &[&[&str]] = &[
    &["features", "shell_tool"],
    &["features", "view_image"],
    &["features", "goals"],
    &["tools", "update_plan", "enabled"],
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    // The original receipt had two entries; length distinguishes newly added
    // policy from a setting whose ownership the user already revoked.
    settings: Vec<Option<Setting>>,
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
    let known = previous.map_or(0, |receipt| receipt.settings.len());
    receipt.settings.resize(SETTINGS.len(), None);
    for (index, path) in SETTINGS.iter().enumerate() {
        let source = Source::parse(&bytes, location)?;
        if index < known && !reinstall {
            // What is owned is the value written. A configuration manager
            // that rewrites the whole file keeps the `false` and changes
            // the bytes around it; only the user turning it back on, or
            // removing it, gives it up.
            if source.item(path).and_then(Item::as_bool) != Some(false) {
                receipt.settings[index] = None;
                notices.push(notice(location, path, "changed since apply; left untouched"));
            }
            continue;
        }
        let value = source
            .item(path)
            .map(|item| item.as_bool().ok_or_else(|| Error::at(path.join("."), "expected a boolean")))
            .transpose()?;
        match value {
            Some(false) => {}
            Some(true) => {
                let range = source.value_range(path)?.expect("the value exists");
                receipt.settings[index] = Some(Setting::Replaced { before: bytes[range.clone()].to_vec() });
                bytes = splice(&bytes, vec![(range, b"false".to_vec())])?;
            }
            None => {
                let (next, setting) = insert(&source, path)?;
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
    for (path, setting) in SETTINGS.iter().zip(&receipt.settings).rev() {
        let (key, parent) = path.split_last().expect("setting path");
        let Some(setting) = setting else {
            continue;
        };
        let source = Source::parse(&bytes, location)?;
        let range = match owned_range(&source, path, setting) {
            Ok(range) => range,
            Err(_) => {
                match rewritten(&source, path, setting)? {
                    Some(next) => {
                        bytes = next;
                        Source::parse(&bytes, location)?;
                    }
                    None => notices.push(notice(location, path, "changed since apply; left untouched")),
                }
                continue;
            }
        };
        if matches!(setting, Setting::Inserted { .. }) && edit::joins_lines(&bytes, &range) {
            bytes = rewritten(&source, path, setting)?.expect("owned value was checked");
            continue;
        }
        let mut edits = Vec::new();
        let replacement = match setting {
            Setting::Replaced { before } => before.clone(),
            Setting::Inserted { inline: true, comma: false, .. } => {
                let table = source
                    .item(parent)
                    .and_then(Item::as_inline_table)
                    .ok_or_else(|| Error::at(parent.join("."), "the inserted inline table member changed"))?;
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
            rewritten(&source, path, setting)?.ok_or_else(|| Error::at(*key, "the recorded value changed"))?
        };
    }
    Ok((bytes, notices))
}

fn owned_range(source: &Source<'_>, path: &[&str], setting: &Setting) -> Result<std::ops::Range<usize>, Error> {
    let (key, parent) = path.split_last().expect("setting path");
    let range = source
        .value_range(path)?
        .filter(|range| source.bytes[range.clone()] == *b"false")
        .ok_or_else(|| Error::at(*key, "the recorded value changed"))?;
    match setting {
        Setting::Replaced { .. } => Ok(range),
        Setting::Inserted { prefix, suffix, inline, .. } => {
            if *inline != source.item(parent).is_some_and(Item::is_inline_table) {
                return Err(Error::at(*key, "the assignment's table layout changed"));
            }
            let start =
                range.start.checked_sub(prefix.len()).ok_or_else(|| Error::at(*key, "the assignment changed"))?;
            let end = range.end + suffix.len();
            if source.bytes.get(start..range.start) != Some(prefix.as_slice())
                || source.bytes.get(range.end..end) != Some(suffix.as_slice())
            {
                return Err(Error::at(*key, "the inserted assignment changed"));
            }
            Ok(start..end)
        }
    }
}

fn insert(source: &Source<'_>, path: &[&str]) -> Result<(Vec<u8>, Setting), Error> {
    let nl = newline(source.bytes);
    for length in (0..path.len()).rev() {
        let Some(parent) = source.item(&path[..length]) else { continue };
        let keys = path[length..].join(".");
        let (at, prefix, suffix, inline, comma) = match parent {
            Item::Value(toml_edit::Value::InlineTable(table)) => {
                let last = table
                    .get_values()
                    .into_iter()
                    .map(|(_, value)| source.span(value.span()).map(|span| span.end))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max();
                let at = last.unwrap_or(source.span(table.span())?.start + 1);
                (
                    at,
                    format!("{}{keys} = ", if last.is_some() { ", " } else { "" }),
                    String::new(),
                    true,
                    last.is_some(),
                )
            }
            Item::Table(_) if length == 0 => (source.root_offset(), format!("{keys} = "), nl.to_owned(), false, false),
            Item::Table(table) if !table.is_implicit() && !table.is_dotted() => {
                let start = source.span(table.span())?.start;
                let (at, prefix) = match source.bytes[start..].iter().position(|b| *b == b'\n') {
                    Some(offset) => (start + offset + 1, format!("{keys} = ")),
                    None => (source.bytes.len(), format!("{nl}{keys} = ")),
                };
                (at, prefix, nl.to_owned(), false, false)
            }
            Item::Table(_) => continue,
            _ => return Err(Error::at(path[..length].join("."), "expected a table")),
        };
        let inserted = format!("{prefix}false{suffix}").into_bytes();
        let bytes = splice(source.bytes, vec![(at..at, inserted)])?;
        Source::parse(&bytes, "config.toml")?;
        return Ok((
            bytes,
            Setting::Inserted { prefix: prefix.into_bytes(), suffix: suffix.into_bytes(), inline, comma },
        ));
    }
    Err(Error::at(path.join("."), "missing root table"))
}

/// Restore a setting still `false` using the current syntax, including inline
/// and dotted tables. A different value is the user's and remains untouched.
fn rewritten(source: &Source<'_>, path: &[&str], setting: &Setting) -> Result<Option<Vec<u8>>, Error> {
    if source.item(path).and_then(Item::as_bool) != Some(false) {
        return Ok(None);
    }
    match setting {
        Setting::Replaced { before } => {
            source.value_range(path)?.map(|range| splice(source.bytes, vec![(range, before.clone())])).transpose()
        }
        Setting::Inserted { .. } => edit::remove(source, path).map(Some),
    }
}

pub(crate) fn plan_configured(bytes: &[u8]) -> Result<bool, Error> {
    let source = Source::parse(bytes, "config.toml")?;
    Ok(source.item(&["tools", "update_plan", "enabled"]).and_then(Item::as_bool) == Some(false))
}

fn notice(location: &str, path: &[&str], reason: &str) -> Notice {
    Notice { location: format!("{location}: {}", path.join(".")), reason: reason.into() }
}
