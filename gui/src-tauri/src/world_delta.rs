//! What changed in the World between two pieces of a watch, for the page to
//! put into the one it holds (`world-gui.md`, 界面): a network of thousands
//! of machines is several megabytes as a whole, and most changes touch a
//! line of it. An object's fields are changed one by one; a list whose
//! entries each have a name of their own (`name`, `machine`, `hub`, ...)
//! entry by entry, and within an entry the same way; anything else is sent
//! as it is now.
//!
//! The change is JSON: `{"v": value}` is the value now; `{"o": {field:
//! change}, "x": [fields gone]}` changes an object; `{"l": {"k": key, "c":
//! {name: change}, "a": [entries added], "x": [names gone], "order":
//! [names]}}` changes a list, `order` sent when it is not the old order with
//! the added entries after it.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

/// The fields that may name a list's entries, tried in this order.
const KEYS: &[&str] = &["name", "machine", "hub", "id", "town", "key", "account", "resident"];

/// The change that makes `old` into `new`; none when they are the same.
pub(crate) fn between(old: &Value, new: &Value) -> Option<Value> {
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
