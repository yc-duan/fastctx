//! The limits the World raises in Claude Code's `env` are managed here: a
//! tool answer's (`MAX_MCP_OUTPUT_TOKENS`) and a tool description's
//! (`CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH`), each with its own receipt.
//! Inputs must be read after the last plugin command. Returned bytes are
//! published by the caller using compare-and-swap against those inputs; this
//! module does no file I/O.

use jsonc_parser::ast::{Object, ObjectProp};
use jsonc_parser::common::Ranged;
use serde::{Deserialize, Serialize};

use super::jsonc::{Document, Insertion};
use crate::plan::ClaudePlan;

/// One limit in `env`: its name, and how high the World needs it.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub key: &'static str,
    pub wanted: u64,
}

impl Limit {
    /// The limit on a tool's answer.
    pub fn output(plan: &ClaudePlan) -> Self {
        Self { key: "MAX_MCP_OUTPUT_TOKENS", wanted: plan.max_mcp_output_tokens }
    }

    /// The limit on a tool's description.
    pub fn description(plan: &ClaudePlan) -> Self {
        Self { key: "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH", wanted: plan.max_mcp_description_length }
    }
}

/// The original value and only the scaffolding created by this edit. This
/// never stores unrelated settings, which can contain secrets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsReceipt {
    before: Option<String>,
    written: String,
    created_env: bool,
    insertion: Option<Insertion>,
    document: Option<NewDocument>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct NewDocument {
    existed: bool,
    prefix: String,
    suffix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsOutcome {
    Written,
    AlreadyApplied,
    Sufficient,
    Restored,
    /// A later user edit or deletion is kept; the old receipt can be retired.
    UserChanged,
    Unmanaged,
}

#[derive(Clone, Debug)]
pub struct SettingsEdit {
    /// `None` means the file should not exist. Compare with the input to
    /// distinguish deletion from an unchanged absent file.
    pub bytes: Option<Vec<u8>>,
    pub receipt: Option<SettingsReceipt>,
    pub outcome: SettingsOutcome,
}

/// Raises the limit on a tool's answer only when necessary. Reapplying
/// preserves the original receipt. A later user edit is reported and never
/// reclaimed or overwritten.
pub fn apply(
    original: Option<&[u8]>,
    plan: &ClaudePlan,
    previous: Option<&SettingsReceipt>,
    take_back: bool,
) -> Result<SettingsEdit, String> {
    apply_limit(original, Limit::output(plan), previous, take_back)
}

/// Raises `limit` only when necessary, as `apply` does the answer's. With
/// `take_back`, a value the user changed or removed since is written again
/// where it no longer allows as much.
pub fn apply_limit(
    original: Option<&[u8]>,
    limit: Limit,
    previous: Option<&SettingsReceipt>,
    take_back: bool,
) -> Result<SettingsEdit, String> {
    let key = limit.key;
    let document = Document::parse(original.unwrap_or_default())?;
    let current = leaf(&document, key)?;
    if let Some(previous) = previous
        && (!matches_written(&document, current, previous)
            || previous.written.parse::<u64>().map_err(|_| "Invalid Claude settings receipt")? >= limit.wanted)
        && (!take_back || matches_written(&document, current, previous))
    {
        return Ok(SettingsEdit {
            bytes: original.map(<[u8]>::to_vec),
            receipt: Some(previous.clone()),
            outcome: if matches_written(&document, current, previous) {
                SettingsOutcome::AlreadyApplied
            } else {
                SettingsOutcome::UserChanged
            },
        });
    }
    if let Some(current) = current
        && super::limit::effective(&current.value)? >= limit.wanted as f64
    {
        return Ok(SettingsEdit {
            bytes: original.map(<[u8]>::to_vec),
            receipt: None,
            outcome: SettingsOutcome::Sufficient,
        });
    }
    let written = limit.wanted.to_string();
    let raw = serde_json::to_string(&written).expect("a string serializes");
    let mut receipt = SettingsReceipt {
        before: current.map(|p| document.raw(&p.value).to_owned()),
        written,
        created_env: false,
        insertion: None,
        document: None,
    };
    if let Some(previous) = previous {
        receipt = SettingsReceipt { written: receipt.written, ..previous.clone() };
    }
    let bytes = if let Some(current) = current {
        let range = current.value.range();
        document.replace(range.start..range.end, &raw)?
    } else if let Some(root) = &document.root {
        let (parent, name, value) = match env(root)? {
            Some(env) => (env, key, raw),
            None => {
                receipt.created_env = true;
                (root, "env", format!("{{\"{key}\": {raw}}}"))
            }
        };
        let (bytes, insertion) = document.insert(parent, name, &value)?;
        receipt.insertion = Some(insertion);
        bytes
    } else {
        let prefix =
            if document.source.is_empty() || document.source.ends_with('\n') || document.source.ends_with('\u{feff}') {
                ""
            } else {
                "\n"
            };
        let text = format!("{prefix}{{\n  \"env\": {{\n    \"{key}\": {raw}\n  }}\n}}\n");
        receipt.created_env = true;
        receipt.document =
            Some(NewDocument { existed: original.is_some(), prefix: prefix.into(), suffix: "\n".into() });
        document.replace(document.source.len()..document.source.len(), &text)?
    };
    Ok(SettingsEdit { bytes: Some(bytes), receipt: Some(receipt), outcome: SettingsOutcome::Written })
}

/// Restores the old raw value of the answer's limit only while the current
/// JSON string is still ours. Later edits elsewhere, including Claude's own
/// plugin settings, stay.
pub fn unapply(original: Option<&[u8]>, receipt: Option<&SettingsReceipt>) -> Result<SettingsEdit, String> {
    unapply_limit(original, "MAX_MCP_OUTPUT_TOKENS", receipt)
}

/// Restores the limit named `key`, as `unapply` does the answer's.
pub fn unapply_limit(
    original: Option<&[u8]>,
    key: &str,
    receipt: Option<&SettingsReceipt>,
) -> Result<SettingsEdit, String> {
    let unchanged = |outcome| SettingsEdit { bytes: original.map(<[u8]>::to_vec), receipt: None, outcome };
    let Some(receipt) = receipt else {
        return Ok(unchanged(SettingsOutcome::Unmanaged));
    };
    let document = Document::parse(original.unwrap_or_default())?;
    let current = leaf(&document, key)?;
    if !matches_written(&document, current, receipt) {
        return Ok(unchanged(SettingsOutcome::UserChanged));
    }
    let current = current.expect("the written value exists");
    let root = document.root.as_ref().expect("the written value has a root");
    let parent = env(root)?.expect("the written value has an env object");
    let bytes = if let Some(before) = &receipt.before {
        let range = current.value.range();
        document.replace(range.start..range.end, before)?
    } else if receipt.created_env
        && parent.properties.len() == 1
        && !document.has_comments(parent.range.start..parent.range.end)
    {
        if let Some(new) = &receipt.document
            && root.properties.len() == 1
            && !document.has_comments(root.range.start..root.range.end)
        {
            let start = root.range.start;
            let end = root.range.end;
            let before = start
                .checked_sub(new.prefix.len())
                .filter(|&i| document.source.get(i..start) == Some(&new.prefix))
                .unwrap_or(start);
            let after = if document.source.get(end..end + new.suffix.len()) == Some(&new.suffix) {
                end + new.suffix.len()
            } else {
                end
            };
            let bytes = document.replace(before..after, "")?;
            return Ok(SettingsEdit {
                bytes: if !new.existed && bytes.is_empty() { None } else { Some(bytes) },
                receipt: None,
                outcome: SettingsOutcome::Restored,
            });
        }
        document.remove(root, root.get("env").expect("env exists"), receipt.insertion.as_ref())?
    } else {
        document.remove(parent, current, if receipt.created_env { None } else { receipt.insertion.as_ref() })?
    };
    Ok(SettingsEdit { bytes: Some(bytes), receipt: None, outcome: SettingsOutcome::Restored })
}

fn env<'a, 'b>(root: &'a Object<'b>) -> Result<Option<&'a Object<'b>>, String> {
    root.get("env")
        .map(|p| p.value.as_object().ok_or_else(|| "Claude settings env must be an object".into()))
        .transpose()
}

fn leaf<'a, 'b>(document: &'a Document<'b>, key: &str) -> Result<Option<&'a ObjectProp<'b>>, String> {
    Ok(document
        .root
        .as_ref()
        .and_then(|root| root.get("env"))
        .and_then(|p| p.value.as_object())
        .and_then(|object| object.get(key)))
}

fn matches_written(document: &Document<'_>, current: Option<&ObjectProp<'_>>, receipt: &SettingsReceipt) -> bool {
    current.is_some_and(|p| {
        serde_json::from_str::<String>(document.raw(&p.value)).ok().as_deref() == Some(&receipt.written)
    })
}

/// Deletes a file created by this apply only after its own entries are gone.
/// Claude's public uninstall commands can leave empty plugin containers.
pub(crate) fn discard_empty_created(bytes: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
    let Some(bytes) = bytes else { return Ok(None) };
    let document = Document::parse(&bytes)?;
    let empty = document.root.as_ref().is_none_or(|root| {
        root.properties.iter().all(|property| {
            matches!(property.name.as_str(), "enabledPlugins" | "extraKnownMarketplaces")
                && property.value.as_object().is_some_and(|object| object.properties.is_empty())
        })
    });
    if empty && !document.has_comments(0..document.source.len()) { Ok(None) } else { Ok(Some(bytes)) }
}
