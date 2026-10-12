//! Claude Code's record that the user accepted its bypass-permissions dialog:
//! `skipDangerousModePermissionPrompt` in the user's settings. Claude Code
//! starts a background session in that mode only once it is there. The edit
//! is ranged, so every other byte of the settings stays as it was.
//!
//! Putting the World's tools into a login grants it, with a receipt of what
//! was there before; taking them out restores that while the key still says
//! what the World wrote (`world-projects.md`, the permission is given with
//! the apply). Where the settings said so already, nothing is written and
//! nothing is the World's to take back.

use jsonc_parser::common::Ranged;
use serde::{Deserialize, Serialize};

use super::jsonc::{Document, Insertion};

const KEY: &str = "skipDangerousModePermissionPrompt";

/// Whether the settings say the user accepted. No settings, no key, or any
/// value but `true` means not accepted.
pub fn accepted(settings: Option<&[u8]>) -> Result<bool, String> {
    let Some(bytes) = settings else { return Ok(false) };
    let document = Document::parse(bytes)?;
    Ok(document
        .root
        .as_ref()
        .and_then(|root| root.get(KEY))
        .is_some_and(|property| document.raw(&property.value) == "true"))
}

/// The settings saying `accept`, or `None` when they already say so.
/// Withdrawing where nothing was accepted changes nothing.
pub fn set(settings: Option<&[u8]>, accept: bool) -> Result<Option<Vec<u8>>, String> {
    if accepted(settings)? == accept {
        return Ok(None);
    }
    let document = Document::parse(settings.unwrap_or_default())?;
    let raw = if accept { "true" } else { "false" };
    let root = document.root.as_ref();
    if let Some(current) = root.and_then(|root| root.get(KEY)) {
        let range = current.value.range();
        return document.replace(range.start..range.end, raw).map(Some);
    }
    if let Some(root) = root {
        return document.insert(root, KEY, raw).map(|(bytes, _)| Some(bytes));
    }
    // No settings yet, or only whitespace and comments.
    let prefix = if document.source.is_empty() || document.source.ends_with('\n') { "" } else { "\n" };
    let text = format!("{prefix}{{\n  \"{KEY}\": {raw}\n}}\n");
    document.replace(document.source.len()..document.source.len(), &text).map(Some)
}

/// What granting wrote over: the key's raw value before, or how it was
/// inserted where it was absent. It never holds any other setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    before: Option<String>,
    insertion: Option<Insertion>,
    /// The settings file did not exist, or held no object, before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    created_document: bool,
}

/// The settings once granted, and the receipt; no receipt when they said so
/// already. A previous receipt is kept while the key still says `true`.
pub fn grant(
    settings: Option<&[u8]>,
    previous: Option<&Receipt>,
) -> Result<(Option<Vec<u8>>, Option<Receipt>), String> {
    if accepted(settings)? {
        return Ok((settings.map(<[u8]>::to_vec), previous.cloned()));
    }
    let document = Document::parse(settings.unwrap_or_default())?;
    let root = document.root.as_ref();
    if let Some(current) = root.and_then(|root| root.get(KEY)) {
        let before = document.raw(&current.value).to_string();
        let range = current.value.range();
        let bytes = document.replace(range.start..range.end, "true")?;
        return Ok((Some(bytes), Some(Receipt { before: Some(before), insertion: None, created_document: false })));
    }
    if let Some(root) = root {
        let (bytes, insertion) = document.insert(root, KEY, "true")?;
        return Ok((Some(bytes), Some(Receipt { before: None, insertion: Some(insertion), created_document: false })));
    }
    let bytes = set(settings, true)?.expect("an absent key is set");
    Ok((Some(bytes), Some(Receipt { before: None, insertion: None, created_document: true })))
}

/// The settings with what granting wrote taken back, and whether the key
/// was left because the user changed it since.
pub fn restore(settings: Option<&[u8]>, receipt: &Receipt) -> Result<(Option<Vec<u8>>, bool), String> {
    let unchanged = (settings.map(<[u8]>::to_vec), true);
    let Some(bytes) = settings else { return Ok(unchanged) };
    let document = Document::parse(bytes)?;
    let Some(root) = document.root.as_ref() else { return Ok(unchanged) };
    let Some(current) = root.get(KEY) else { return Ok(unchanged) };
    if document.raw(&current.value) != "true" {
        return Ok(unchanged);
    }
    if let Some(before) = &receipt.before {
        let range = current.value.range();
        return Ok((Some(document.replace(range.start..range.end, before)?), false));
    }
    if receipt.created_document && root.properties.len() == 1 && !document.has_comments(0..document.source.len()) {
        return Ok((None, false));
    }
    Ok((Some(document.remove(root, current, receipt.insertion.as_ref())?), false))
}
