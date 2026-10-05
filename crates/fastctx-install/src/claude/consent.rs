//! Claude Code's record that the user accepted its bypass-permissions dialog:
//! `skipDangerousModePermissionPrompt` in the user's settings. Claude Code
//! starts a background session in that mode only once it is there. The edit
//! is ranged, so every other byte of the settings stays as it was.

use jsonc_parser::common::Ranged;

use super::jsonc::Document;

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
