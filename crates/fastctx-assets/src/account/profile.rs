//! Profile payloads omit version counters and retain unknown fields losslessly.
use crate::{Error, json};
use fastctx_types::profile::{Entry, Profile};
use serde_json::{Map, Value};

pub fn decode(body: &[u8]) -> Result<Profile, Error> {
    let mut value = json::object(body)?;
    json::no_version(&value)?;
    if let Some(layer) = value.get("layer").filter(|v| !v.is_null()) {
        let _: fastctx_types::layers::Layer = json::typed(layer.clone())?;
    }
    if let Some(servers) = value.get("servers").and_then(Value::as_object) {
        for server in servers.values() {
            let server = server.as_object().ok_or_else(|| json::invalid("MCP entry must be an object"))?;
            match server.get("kind").and_then(Value::as_str) {
                Some("command") => json::exact(server, &["kind", "machine"])?,
                Some("remote") => json::exact(server, &["kind", "url", "sse", "headers"])?,
                _ => return Err(json::invalid("unsupported MCP entry kind")),
            }
        }
    }
    value.insert("version".into(), Value::from(0));
    let profile: Profile = json::typed(Value::Object(value))?;
    profile.validate().map_err(Error::Invalid)?;
    Ok(profile)
}
pub(crate) fn semantic(body: &[u8]) -> Result<Value, Error> {
    let mut value = if body.is_empty() { Map::new() } else { json::object(body)? };
    let defaults = serde_json::json!({"memory":true,"layer":null,"prompt":"","prompts":null,"claude":{},"codex":{},"servers":{},"subagents":null});
    for (key, default) in defaults.as_object().expect("object") {
        value.entry(key).or_insert_with(|| default.clone());
    }
    Ok(Value::Object(value))
}

/// Converts only a classified public Profile. The migration envelope retains
/// its original version, actor and receipt independently of this payload.
pub fn import(body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut value = json::object(body)?;
    let _: Profile = json::typed(Value::Object(value.clone()))?;
    value.remove("version");
    let result = json::bytes(&value)?;
    decode(&result)?;
    Ok(result)
}

/// A trusted compatibility adapter supplies its mapped legacy version. This
/// function neither derives that mapping nor grants permission to read a profile.
pub fn project(body: &[u8], legacy_version: u64) -> Result<Profile, Error> {
    let mut profile = decode(body)?;
    profile.version = legacy_version;
    Ok(profile)
}

/// Old Profile writes preserve dedicated fields and fields the old caller did
/// not understand. MCP writes use their separate, authorized preparation path.
pub fn apply_legacy_write(current: &[u8], incoming: &[u8]) -> Result<Vec<u8>, Error> {
    let mut kept = json::object(current)?;
    let incoming = json::object(incoming)?;
    let parsed: Profile = json::typed(Value::Object(incoming.clone()))?;
    // The old hub deserialized Entry before saving it; command execution
    // details ignored by that public shape must not enter an asset now.
    kept.insert(
        "servers".into(),
        serde_json::to_value(parsed.servers).map_err(|_| json::invalid("invalid MCP servers"))?,
    );
    for key in ["prompt", "claude", "codex"] {
        match incoming.get(key) {
            Some(value) => {
                kept.insert(key.into(), value.clone());
            }
            None => {
                kept.remove(key);
            }
        }
    }
    if let Some(prompts) = incoming.get("prompts").filter(|v| !v.is_null()) {
        // Preserve fields added by newer family-specific prompt schemas.
        let mut target = kept.get("prompts").and_then(Value::as_object).cloned().unwrap_or_default();
        let prompts = prompts.as_object().ok_or_else(|| json::invalid("invalid prompts"))?;
        for key in ["claude", "codex"] {
            match prompts.get(key) {
                Some(value) => {
                    target.insert(key.into(), value.clone());
                }
                None => {
                    target.remove(key);
                }
            }
        }
        kept.insert("prompts".into(), Value::Object(target));
    }
    let out = json::bytes(&kept)?;
    decode(&out)?;
    Ok(out)
}

pub(crate) fn command_targets(before: Option<&Profile>, after: &Profile) -> Vec<fastctx_types::MachineName> {
    after
        .servers
        .iter()
        .filter_map(|(name, entry)| {
            let Entry::Command { machine } = entry else { return None };
            if before.and_then(|p| p.servers.get(name)) == Some(entry) { None } else { Some(machine.clone()) }
        })
        .collect()
}
