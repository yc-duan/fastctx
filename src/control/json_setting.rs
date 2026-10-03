//! Inverse edits for individual shared JSON settings.

use super::settings::JsoncConfigReceipt;
use super::targets::json_config;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JsonSettingReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub applied_hash: String,
    pub insertion: JsoncConfigReceipt,
}

pub(crate) fn set(
    original: &[u8],
    path: &[&str],
    raw: &str,
    previous: Option<&JsonSettingReceipt>,
) -> Result<(Vec<u8>, JsonSettingReceipt), String> {
    let before = json_config::raw_value(original, path)?;
    let hash = json_config::inspect(original, path)?;
    if let Some(previous) = previous
        && hash.as_deref() != Some(&previous.applied_hash)
    {
        return Err(format!(
            "Managed JSONC entry {} drifted after Apply; FastCtx will not overwrite user-changed bytes.",
            path.join(".")
        ));
    }
    let desired = serde_json::from_str(raw)
        .map_err(|error| format!("Cannot parse JSON setting {}: {error}", path.join(".")))?;
    let edit = json_config::apply(
        original,
        path,
        &desired,
        hash.as_deref(),
        previous.map(|r| &r.insertion),
    )?;
    let bytes = json_config::replace_raw(&edit.bytes, path, raw)?;
    Ok((
        bytes,
        JsonSettingReceipt {
            before: previous.map(|r| r.before.clone()).unwrap_or(before),
            applied_hash: edit.managed_hash,
            insertion: edit.receipt,
        },
    ))
}

pub(crate) fn restore(
    original: &[u8],
    path: &[&str],
    receipt: &JsonSettingReceipt,
) -> Result<Vec<u8>, String> {
    if json_config::inspect(original, path)?.as_deref() != Some(&receipt.applied_hash) {
        return Err(format!(
            "Managed JSONC entry {} drifted after Apply; FastCtx will not delete user-changed bytes.",
            path.join(".")
        ));
    }
    match &receipt.before {
        Some(raw) => json_config::replace_raw(original, path, raw),
        None => json_config::disconnect(
            original,
            path,
            &receipt.applied_hash,
            Some(&receipt.insertion),
        ),
    }
}
