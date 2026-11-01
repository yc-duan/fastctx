//! What changed in the World between two pieces of a watch, for whoever
//! holds the first to put into it (`world-gui.md`, 界面): a network of
//! thousands of machines is several megabytes as a whole, and most changes
//! touch a line of it. The node tells an interface what changed, and the
//! interface tells its page. An object's fields are changed one by one; a
//! list whose entries each have a name of their own (`name`, `machine`,
//! `hub`, ...) entry by entry, and within an entry the same way; anything
//! else is sent as it is now.
//!
//! The change is JSON: `{"v": value}` is the value now; `{"o": {field:
//! change}, "x": [fields gone]}` changes an object; `{"l": {"k": key, "c":
//! {name: change}, "a": [entries added], "x": [names gone], "order":
//! [names]}}` changes a list, `order` sent when it is not the old order with
//! the added entries after it. The page puts it in as `apply` does
//! (`gui/src/world/change.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

/// The fields that may name a list's entries, tried in this order.
const KEYS: &[&str] = &["name", "machine", "hub", "id", "town", "key", "account", "resident"];

/// What tells the World now to one that holds an earlier piece of it.
#[derive(Clone, Debug, PartialEq)]
pub enum Told {
    /// The whole World.
    Whole(Value),
    /// What changed in it.
    Changed(Value),
}

/// What tells `now` to one that holds `held`: what changed, or the whole
/// when that is no smaller than half of it; none when nothing did. `held`
/// becomes `now`.
pub fn tell(held: &mut Option<Value>, now: Value) -> Option<Told> {
    let told = match held.as_ref() {
        Some(before) => match between(before, &now) {
            None => None,
            Some(change) if length(&change) < length(&now) / 2 => Some(Told::Changed(change)),
            Some(_) => Some(Told::Whole(now.clone())),
        },
        None => Some(Told::Whole(now.clone())),
    };
    *held = Some(now);
    told
}

fn length(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

/// The change that makes `old` into `new`; none when they are the same.
pub fn between(old: &Value, new: &Value) -> Option<Value> {
    (old != new).then(|| change(old, new))
}

fn change(old: &Value, new: &Value) -> Value {
    match (old, new) {
        (Value::Object(old), Value::Object(new)) => {
            let mut fields = Map::new();
            for (name, value) in new {
                match old.get(name) {
                    Some(before) if before == value => {}
                    Some(before) => {
                        fields.insert(name.clone(), change(before, value));
                    }
                    None => {
                        fields.insert(name.clone(), json!({ "v": value }));
                    }
                }
            }
            let gone: Vec<Value> =
                old.keys().filter(|name| !new.contains_key(*name)).map(|name| Value::from(name.clone())).collect();
            let mut out = Map::new();
            if !fields.is_empty() {
                out.insert("o".into(), Value::Object(fields));
            }
            if !gone.is_empty() {
                out.insert("x".into(), Value::Array(gone));
            }
            Value::Object(out)
        }
        (Value::Array(old), Value::Array(new)) => match key_of(old, new) {
            Some(key) => list(old, new, key),
            None => json!({ "v": new }),
        },
        _ => json!({ "v": new }),
    }
}

/// The field that names each entry of both lists alike, when one does.
fn key_of(old: &[Value], new: &[Value]) -> Option<&'static str> {
    if old.is_empty() || new.is_empty() {
        return None;
    }
    KEYS.iter().copied().find(|key| named(old, key) && named(new, key))
}

/// Whether each entry has a name of its own under `key`.
fn named(entries: &[Value], key: &str) -> bool {
    let mut seen = HashSet::new();
    entries.iter().all(|entry| entry.get(key).and_then(Value::as_str).is_some_and(|name| seen.insert(name)))
}

fn list(old: &[Value], new: &[Value], key: &'static str) -> Value {
    let name = |entry: &Value| entry.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let before: HashMap<String, &Value> = old.iter().map(|entry| (name(entry), entry)).collect();
    let now: HashSet<String> = new.iter().map(name).collect();
    let mut changed = Map::new();
    let mut added = Vec::new();
    for entry in new {
        match before.get(&name(entry)) {
            Some(was) if *was == entry => {}
            Some(was) => {
                changed.insert(name(entry), change(was, entry));
            }
            None => added.push(entry.clone()),
        }
    }
    let gone: Vec<Value> = old.iter().map(name).filter(|one| !now.contains(one)).map(Value::from).collect();
    let mut out = Map::new();
    out.insert("k".into(), key.into());
    if !changed.is_empty() {
        out.insert("c".into(), Value::Object(changed));
    }
    if !added.is_empty() {
        out.insert("a".into(), Value::Array(added.clone()));
    }
    if !gone.is_empty() {
        out.insert("x".into(), Value::Array(gone));
    }
    let order: Vec<String> = new.iter().map(name).collect();
    let expected: Vec<String> =
        old.iter().map(name).filter(|one| now.contains(one)).chain(added.iter().map(name)).collect();
    if order != expected {
        out.insert("order".into(), order.into());
    }
    json!({ "l": out })
}

/// Puts `change` into `held`, as the page does.
pub fn apply(held: &mut Value, change: &Value) {
    if let Some(now) = change.get("v") {
        *held = now.clone();
        return;
    }
    if let Some(list) = change.get("l") {
        let key = list.get("k").and_then(Value::as_str).unwrap_or_default();
        let name = |entry: &Value| match entry.get(key) {
            Some(Value::String(name)) => name.clone(),
            Some(other) => other.to_string(),
            None => "undefined".to_string(),
        };
        let gone: HashSet<String> =
            list.get("x").and_then(Value::as_array).into_iter().flatten().filter_map(as_name).collect();
        let changes = list.get("c").and_then(Value::as_object);
        let mut all: Vec<Value> = match std::mem::take(held) {
            Value::Array(entries) => entries,
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|entry| !gone.contains(&name(entry)))
        .map(|mut entry| {
            if let Some(changed) = changes.and_then(|changes| changes.get(&name(&entry))) {
                apply(&mut entry, changed);
            }
            entry
        })
        .collect();
        all.extend(list.get("a").and_then(Value::as_array).into_iter().flatten().cloned());
        *held = match list.get("order").and_then(Value::as_array) {
            None => Value::Array(all),
            Some(order) => {
                let mut by_name: HashMap<String, Value> = all.into_iter().map(|entry| (name(&entry), entry)).collect();
                Value::Array(
                    order.iter().filter_map(as_name).map(|one| by_name.remove(&one).unwrap_or(Value::Null)).collect(),
                )
            }
        };
        return;
    }
    if !held.is_object() {
        *held = Value::Object(Map::new());
    }
    let fields = held.as_object_mut().expect("an object");
    for (field, inner) in change.get("o").and_then(Value::as_object).into_iter().flatten() {
        apply(fields.entry(field.clone()).or_insert(Value::Null), inner);
    }
    for field in change.get("x").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        fields.remove(field);
    }
}

fn as_name(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}
