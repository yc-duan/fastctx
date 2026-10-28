//! Own only the deny entries added by apply, never the user's permission list.

use jsonc_parser::ast::{Array, Object, Value};
use jsonc_parser::common::Ranged;
use serde::{Deserialize, Serialize};

use super::jsonc::{Document, Insertion};
use crate::{File, Notice};

/// Every deny rule apply has ever owned, in the order receipts record
/// them. Only the host's shell is denied in the user's own login now
/// (`world-layers.md`): the World's `run` takes its place there, and the
/// rest stays the user's; the World's own conversations deny more in their
/// launch settings. The others are kept so older receipts still line up,
/// and what an earlier apply added of them is withdrawn.
pub const DENIED: [&str; 10] = [
    "Bash",
    "Grep",
    "Glob",
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
    "ProposeGoal",
    "PowerShell",
];

/// Whether the rule at `index` of `DENIED` is denied now: the shell, and
/// on Windows the shell Claude Code offers there as well.
fn wanted(index: usize) -> bool {
    DENIED[index] == "Bash" || (DENIED[index] == "PowerShell" && cfg!(windows))
}

/// The rules denied now, for what the user is told.
pub fn denied() -> Vec<&'static str> {
    (0..DENIED.len()).filter(|index| wanted(*index)).map(|index| DENIED[index]).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    path: String,
    rules: Vec<Option<Added>>,
    permissions: Option<Insertion>,
    deny: Option<Insertion>,
    document: Option<DocumentInsertion>,
}

impl Receipt {
    pub(crate) fn bind_home(&mut self, home: &std::path::Path) {
        self.path = home.join("settings.json").to_string_lossy().into_owned();
    }

    pub(crate) fn settings_path(&self) -> &std::path::Path {
        std::path::Path::new(&self.path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Added {
    raw: String,
    prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentInsertion {
    existed: bool,
    prefix: String,
}

pub struct Applied {
    /// None preserves a settings file the user deleted after apply.
    pub bytes: Option<Vec<u8>>,
    pub receipt: Receipt,
    pub notices: Vec<Notice>,
}

pub struct Unapplied {
    pub bytes: Option<Vec<u8>>,
    pub notices: Vec<Notice>,
}

/// Add missing bare deny rules on first install. A receipt, including an empty
/// one, prevents subsequent apply calls from reclaiming user edits or deletions,
/// unless they `take_back`: a rule removed since is added again, one the user
/// rewrote that still denies the tool stays theirs. A rule an earlier apply
/// added that is no longer denied is withdrawn while it is as added.
pub fn apply(file: File<'_>, previous: Option<&Receipt>, take_back: bool) -> Result<Applied, String> {
    if let Some(previous) = previous {
        crate::check_path(file.path, &previous.path).map_err(|error| error.to_string())?;
    }
    let mut bytes = file.bytes.unwrap_or_default().to_vec();
    let mut receipt = previous.cloned().unwrap_or_else(|| Receipt {
        path: file.path.into(),
        rules: Vec::new(),
        permissions: None,
        deny: None,
        document: None,
    });
    let known = previous.map_or(0, |receipt| receipt.rules.len());
    receipt.rules.resize(DENIED.len(), None);
    let mut notices = Vec::new();
    if previous.is_some() {
        let document = Document::parse(&bytes)?;
        let array = match deny(&document) {
            Ok(array) => array,
            Err(error) => {
                receipt.rules.fill(None);
                receipt.permissions = None;
                receipt.deny = None;
                receipt.document = None;
                return Ok(Applied {
                    bytes: file.bytes.map(<[u8]>::to_vec),
                    receipt,
                    notices: vec![shape_notice(file.path, &error)],
                });
            }
        };
        for (index, rule) in DENIED.iter().take(known).enumerate() {
            if !wanted(index) {
                continue;
            }
            let matches = matching(array, rule);
            let intact = match &receipt.rules[index] {
                Some(added) => {
                    matches.len() == 1 && document.raw(&array.expect("matched array").elements[matches[0]]) == added.raw
                }
                None => !matches.is_empty(),
            };
            if !intact {
                receipt.rules[index] = None;
                if !take_back || !matches.is_empty() {
                    notices.push(notice(file.path, rule, "changed since apply; left untouched"));
                }
            }
        }
        // What an earlier apply added and is no longer denied goes, unless
        // the user changed it since.
        for index in (0..known.min(DENIED.len())).rev() {
            if wanted(index) {
                continue;
            }
            let Some(added) = receipt.rules[index].take() else { continue };
            let document = Document::parse(&bytes)?;
            let array = deny(&document)?;
            let matches = matching(array, DENIED[index]);
            if matches.len() != 1 || document.raw(&array.expect("matched array").elements[matches[0]]) != added.raw {
                notices.push(notice(file.path, DENIED[index], "changed or duplicated since apply; left untouched"));
                continue;
            }
            bytes = document.remove_element(array.expect("matched array"), matches[0], &added.prefix)?;
        }
        if !take_back && (known >= DENIED.len() || file.bytes.is_none()) {
            let changed = file.bytes.is_some_and(|original| original != bytes.as_slice());
            return Ok(Applied {
                bytes: if changed { Some(bytes) } else { file.bytes.map(<[u8]>::to_vec) },
                receipt,
                notices,
            });
        }
    }
    if Document::parse(&bytes)?.root.is_none() {
        let document = Document::parse(&bytes)?;
        let prefix = if document.source.is_empty() || document.source.ends_with('\n') { "" } else { "\n" };
        bytes = document.replace(document.source.len()..document.source.len(), &format!("{prefix}{{}}\n"))?;
        receipt.document = Some(DocumentInsertion { existed: file.bytes.is_some(), prefix: prefix.into() });
    }
    if permissions(&Document::parse(&bytes)?)?.is_none() {
        let document = Document::parse(&bytes)?;
        let (next, inserted) = document.insert(document.root.as_ref().expect("root created"), "permissions", "{}")?;
        bytes = next;
        receipt.permissions = Some(inserted);
    }
    if deny(&Document::parse(&bytes)?)?.is_none() {
        let document = Document::parse(&bytes)?;
        let (next, inserted) = document.insert(permissions(&document)?.expect("permissions created"), "deny", "[]")?;
        bytes = next;
        receipt.deny = Some(inserted);
    }
    for (index, rule) in DENIED.iter().enumerate() {
        if !wanted(index) || (index < known && !take_back) {
            continue;
        }
        let document = Document::parse(&bytes)?;
        let array = deny(&document)?.expect("deny created");
        if !matching(Some(array), rule).is_empty() {
            continue;
        }
        let at = array.elements.last().map_or(array.range.start + 1, |value| value.range().end);
        let prefix = if array.elements.is_empty() { "" } else { ", " };
        let raw = serde_json::to_string(rule).expect("a rule serializes");
        bytes = document.replace(at..at, &format!("{prefix}{raw}"))?;
        receipt.rules[index] = Some(Added { raw, prefix: prefix.into() });
    }
    Ok(Applied { bytes: Some(bytes), receipt, notices })
}

/// Withdraw only unchanged, uniquely identifiable owned rules and empty
/// scaffolding. Rewritten or duplicated rules and user comments survive.
pub fn unapply(file: File<'_>, receipt: &Receipt) -> Result<Unapplied, String> {
    crate::check_path(file.path, &receipt.path).map_err(|error| error.to_string())?;
    let Some(original) = file.bytes else { return Ok(Unapplied { bytes: None, notices: Vec::new() }) };
    let document = Document::parse(original)?;
    if let Err(error) = deny(&document) {
        return Ok(Unapplied { bytes: Some(original.to_vec()), notices: vec![shape_notice(file.path, &error)] });
    }
    let mut bytes = original.to_vec();
    let mut notices = Vec::new();
    for (rule, added) in DENIED.iter().zip(&receipt.rules).rev() {
        let Some(added) = added else { continue };
        let document = Document::parse(&bytes)?;
        let array = deny(&document)?;
        let matches = matching(array, rule);
        if matches.len() != 1 || document.raw(&array.expect("matched array").elements[matches[0]]) != added.raw {
            notices.push(notice(file.path, rule, "changed or duplicated since apply; left untouched"));
            continue;
        }
        bytes = document.remove_element(array.expect("matched array"), matches[0], &added.prefix)?;
    }
    if let Some(inserted) = &receipt.deny {
        let document = Document::parse(&bytes)?;
        if let Some(array) = deny(&document)?
            && array.elements.is_empty()
            && !document.has_comments(array.range.start..array.range.end)
        {
            let object = permissions(&document)?.expect("deny parent");
            let property = object.properties.iter().find(|p| p.name.as_str() == "deny").expect("deny property");
            bytes = document.remove(object, property, Some(inserted))?;
        }
    }
    if let Some(inserted) = &receipt.permissions {
        let document = Document::parse(&bytes)?;
        if let Some(object) = permissions(&document)?
            && object.properties.is_empty()
            && !document.has_comments(object.range.start..object.range.end)
        {
            let root = document.root.as_ref().expect("permissions parent");
            let property =
                root.properties.iter().find(|p| p.name.as_str() == "permissions").expect("permissions property");
            bytes = document.remove(root, property, Some(inserted))?;
        }
    }
    if let Some(inserted) = &receipt.document {
        let document = Document::parse(&bytes)?;
        if let Some(root) = &document.root
            && root.properties.is_empty()
            && !document.has_comments(root.range.start..root.range.end)
        {
            let start = root
                .range
                .start
                .checked_sub(inserted.prefix.len())
                .filter(|start| document.source.get(*start..root.range.start) == Some(&inserted.prefix))
                .unwrap_or(root.range.start);
            let end = root.range.end + usize::from(document.source[root.range.end..].starts_with('\n'));
            let next = document.replace(start..end, "")?;
            return Ok(Unapplied { bytes: crate::finish(next, inserted.existed), notices });
        }
    }
    Ok(Unapplied { bytes: Some(bytes), notices })
}

/// Check the user settings only; managed policy can exclude user deny rules.
pub fn configured(bytes: Option<&[u8]>) -> Result<bool, String> {
    let document = Document::parse(bytes.unwrap_or_default())?;
    let array = deny(&document)?;
    Ok(denied().iter().all(|rule| !matching(array, rule).is_empty()))
}

fn permissions<'a>(document: &'a Document<'a>) -> Result<Option<&'a Object<'a>>, String> {
    document
        .root
        .as_ref()
        .and_then(|root| root.properties.iter().find(|p| p.name.as_str() == "permissions"))
        .map(|property| match &property.value {
            Value::Object(object) => Ok(object),
            _ => Err("Claude permissions must be an object".into()),
        })
        .transpose()
}

fn deny<'a>(document: &'a Document<'a>) -> Result<Option<&'a Array<'a>>, String> {
    let array = permissions(document)?
        .and_then(|object| object.properties.iter().find(|p| p.name.as_str() == "deny"))
        .map(|property| match &property.value {
            Value::Array(array) => Ok(array),
            _ => Err("Claude permissions.deny must be an array".to_owned()),
        })
        .transpose()?;
    if array.is_some_and(|array| array.elements.iter().any(|value| !matches!(value, Value::StringLit(_)))) {
        return Err("Claude permissions.deny must contain strings".into());
    }
    Ok(array)
}

fn matching(array: Option<&Array<'_>>, rule: &str) -> Vec<usize> {
    array
        .into_iter()
        .flat_map(|array| array.elements.iter().enumerate())
        .filter_map(|(index, value)| match value {
            Value::StringLit(value) if value.value == rule => Some(index),
            _ => None,
        })
        .collect()
}

fn notice(path: &str, rule: &str, reason: &str) -> Notice {
    Notice { location: format!("{path}: permissions.deny[{rule}]"), reason: reason.into() }
}

fn shape_notice(path: &str, error: &str) -> Notice {
    Notice {
        location: format!("{path}: permissions.deny"),
        reason: format!("changed since apply; left untouched ({error})"),
    }
}
