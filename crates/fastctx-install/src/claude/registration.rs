//! Only the reserved product's declarations are removed here. Claude commands
//! can leave them behind without a listable installation record.

use super::jsonc::Document;
use crate::plan::ClaudePlan;

pub(crate) fn remove(
    original: Option<&[u8]>,
    plan: &ClaudePlan,
    plugin: bool,
    marketplace: bool,
) -> Result<Option<Vec<u8>>, String> {
    let Some(original) = original else { return Ok(None) };
    let mut bytes = original.to_vec();
    let id = format!("{}@{}", plan.plugin, plan.marketplace_name);
    for (enabled, container, key) in
        [(plugin, "enabledPlugins", id.as_str()), (marketplace, "extraKnownMarketplaces", plan.marketplace_name)]
    {
        if !enabled {
            continue;
        }
        let document = Document::parse(&bytes)?;
        let Some(root) = &document.root else { continue };
        let Some(property) = root.get(container) else { continue };
        let object = property.value.as_object().ok_or_else(|| format!("Claude {container} must be an object"))?;
        let Some(property) = object.get(key) else { continue };
        if marketplace && container == "extraKnownMarketplaces" {
            let source = property
                .value
                .as_object()
                .and_then(|value| value.get("source"))
                .and_then(|property| property.value.as_object())
                .ok_or("Marketplace declaration has no source object")?;
            let string = |name: &str| -> Result<String, String> {
                let property = source.get(name).ok_or_else(|| format!("Marketplace source has no {name}"))?;
                serde_json::from_str(document.raw(&property.value))
                    .map_err(|_| format!("Marketplace source {name} must be a string"))
            };
            let path = string("path")?;
            let normalize = |path: &str| {
                if cfg!(windows) {
                    path.replace('\\', "/").trim_start_matches("//?/").to_ascii_lowercase()
                } else {
                    path.to_string()
                }
            };
            if string("source")? != "directory" || normalize(&path) != normalize(&plan.marketplace.to_string_lossy()) {
                return Err("The fastctx marketplace declaration points at a different source; left unchanged".into());
            }
        }
        bytes = document.remove(object, property, None)?;
    }
    Ok(Some(bytes))
}

/// Only these two reserved declarations may explain a CLI-induced settings
/// revision during file rollback. Other values and all comments still compare.
pub(crate) fn same_unmanaged(before: Option<&[u8]>, after: Option<&[u8]>) -> Result<bool, String> {
    let comparable = |bytes: Option<&[u8]>| -> Result<_, String> {
        let document = Document::parse(bytes.unwrap_or_default())?;
        let mut value: serde_json::Value = jsonc_parser::parse_to_serde_value(document.source, &Default::default())
            .map_err(|error| format!("reading Claude settings for recovery: {error}"))?;
        if value.is_null() {
            value = serde_json::json!({});
        }
        let root = value.as_object_mut().ok_or("Claude settings must be an object")?;
        for (container, key) in [("enabledPlugins", "fastctx-world@fastctx"), ("extraKnownMarketplaces", "fastctx")] {
            if let Some(object) = root.get_mut(container).and_then(serde_json::Value::as_object_mut) {
                object.remove(key);
                if object.is_empty() {
                    root.remove(container);
                }
            }
        }
        Ok((value, document.comments()))
    };
    Ok(comparable(before)? == comparable(after)?)
}
