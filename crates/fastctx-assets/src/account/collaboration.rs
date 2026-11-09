//! Collaboration preserves stable callee names and legacy unsaid limits.
use crate::{Error, json};
use fastctx_types::collab::{AgentLogin, Collab};
use serde_json::{Map, Value};

pub fn decode(body: &[u8]) -> Result<Collab, Error> {
    let mut value = json::object(body)?;
    json::no_version(&value)?;
    value.insert("version".into(), Value::from(0));
    let collab: Collab = json::typed(Value::Object(value))?;
    collab.validate().map_err(Error::Invalid)?;
    Ok(collab)
}
pub fn import(body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut value = json::object(body)?;
    let _: Collab = json::typed(Value::Object(value.clone()))?;
    value.remove("version");
    let out = json::bytes(&value)?;
    decode(&out)?;
    Ok(out)
}
pub fn project(body: &[u8], legacy_version: u64) -> Result<Collab, Error> {
    let mut collab = decode(body)?;
    collab.version = legacy_version;
    Ok(collab)
}

/// Named entries are compared by their business identities, never array indexes.
pub(crate) fn semantic(body: &[u8]) -> Result<Value, Error> {
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let mut value = json::object(body)?;
    for field in ["callees", "callers"] {
        let entries =
            value.get(field).and_then(Value::as_array).ok_or_else(|| json::invalid("invalid collaboration list"))?;
        let mut keyed = Map::new();
        let mut order = Vec::new();
        for entry in entries {
            let key = if field == "callees" {
                entry.get("name").and_then(Value::as_str).ok_or_else(|| json::invalid("callee has no name"))?.into()
            } else {
                let login: AgentLogin = json::typed(entry.clone())?;
                serde_json::to_string(&login).map_err(|_| json::invalid("invalid caller login"))?
            };
            // Repeated caller logins are legal in the old validator; retain
            // their full array when there is no unique business key.
            order.push(Value::String(key.clone()));
            if keyed.insert(key, entry.clone()).is_some() {
                keyed.clear();
                break;
            }
        }
        if !keyed.is_empty() || entries.is_empty() {
            // This reserved semantic coordinate cannot be a callee name or
            // encoded login, and keeps ordering changes out of array indexes.
            keyed.insert("$order".into(), Value::Array(order));
            value.insert(field.into(), Value::Object(keyed));
        }
    }
    value.entry("prompt").or_insert(Value::Null);
    Ok(Value::Object(value))
}

pub fn apply_legacy_write(current: &[u8], incoming: &[u8]) -> Result<Vec<u8>, Error> {
    let previous = decode(current)?;
    let mut parsed: Collab = json::typed(Value::Object(json::object(incoming)?))?;
    parsed.validate().map_err(Error::Invalid)?;
    parsed.keep_unsaid(Some(&previous));
    let mut next = json::object(current)?;
    let incoming = json::object(incoming)?;
    let mut callers = Vec::new();
    let old_callers = next.get("callers").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut used = vec![false; old_callers.len()];
    for raw in incoming.get("callers").and_then(Value::as_array).ok_or_else(|| json::invalid("missing callers"))? {
        let login: AgentLogin = json::typed(raw.clone())?;
        let found = old_callers.iter().enumerate().find(|(index, value)| {
            !used[*index] && serde_json::from_value::<AgentLogin>((*value).clone()).ok().as_ref() == Some(&login)
        });
        let previous = found.map(|(index, value)| {
            used[index] = true;
            value
        });
        callers.push(merge_login(previous, raw)?);
    }
    next.insert("callers".into(), Value::Array(callers));
    for key in ["on", "prompt", "rules"] {
        next.insert(key.into(), incoming.get(key).cloned().unwrap_or(Value::Null));
    }
    let old_entries = next.get("callees").and_then(Value::as_array);
    let mut callees = Vec::new();
    for (raw, parsed) in incoming
        .get("callees")
        .and_then(Value::as_array)
        .ok_or_else(|| json::invalid("missing callees"))?
        .iter()
        .zip(&parsed.callees)
    {
        let mut entry = old_entries
            .into_iter()
            .flatten()
            .find(|v| v.get("name").and_then(Value::as_str) == Some(&parsed.name))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let raw = raw.as_object().ok_or_else(|| json::invalid("invalid callee"))?;
        let login =
            merge_login(entry.get("login"), raw.get("login").ok_or_else(|| json::invalid("missing callee login"))?)?;
        entry.insert("login".into(), login);
        for key in ["name", "use_when", "model", "effort", "screen"] {
            match raw.get(key) {
                Some(value) => {
                    entry.insert(key.into(), value.clone());
                }
                None => {
                    entry.remove(key);
                }
            }
        }
        entry.insert(
            "at_once".into(),
            serde_json::to_value(parsed.at_once).map_err(|_| json::invalid("invalid concurrency limit"))?,
        );
        callees.push(Value::Object(entry));
    }
    next.insert("callees".into(), Value::Array(callees));
    let result = json::bytes(&next)?;
    decode(&result)?;
    Ok(result)
}

fn merge_login(previous: Option<&Value>, incoming: &Value) -> Result<Value, Error> {
    let mut kept = previous.and_then(Value::as_object).cloned().unwrap_or_default();
    let incoming = incoming.as_object().ok_or_else(|| json::invalid("invalid login"))?;
    for key in ["machine", "agent", "home"] {
        match incoming.get(key) {
            Some(value) => {
                kept.insert(key.into(), value.clone());
            }
            None => {
                kept.remove(key);
            }
        }
    }
    Ok(Value::Object(kept))
}

pub(crate) fn caller_extensions_preserved(before: &[u8], after: &[u8]) -> bool {
    fn entries(body: &[u8]) -> Option<Vec<(AgentLogin, Map<String, Value>)>> {
        if body.is_empty() {
            return Some(Vec::new());
        }
        let value = json::object(body).ok()?;
        value
            .get("callers")?
            .as_array()?
            .iter()
            .map(|entry| {
                let login: AgentLogin = serde_json::from_value(entry.clone()).ok()?;
                let extras = entry
                    .as_object()?
                    .iter()
                    .filter(|(k, _)| !["machine", "agent", "home"].contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Some((login, extras))
            })
            .collect()
    }
    let (Some(before), Some(after)) = (entries(before), entries(after)) else { return false };
    after.iter().all(|(login, extras)| {
        let previous: Vec<_> = before.iter().filter(|(old, _)| old == login).collect();
        if previous.is_empty() { extras.is_empty() } else { previous.iter().any(|(_, kept)| kept == extras) }
    })
}
