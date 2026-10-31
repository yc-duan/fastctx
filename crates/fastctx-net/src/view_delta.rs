//! A member's view sent as what changed since the one it holds
//! (`world-concurrency.md`, 只给有关的，给变化的部分). The hub numbers the
//! views it gives a session (`View::revision`); a change goes as the fields
//! that differ and, of the long lists, the entries that came, changed or
//! went, in the order the whole view has them. A member holding another
//! number than the one a change starts from asks for the whole view.

mod apply;
pub use apply::apply;

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::directory::View;

/// The lists sent entry by entry, each with the fields that name an entry.
const KEYED: &[(&str, &[&str])] = &[
    ("members", &["name"]),
    ("accounts", &["name"]),
    ("projects", &["town"]),
    ("shares", &["machine", "account"]),
    ("lends", &["machine", "account", "agent", "home"]),
    ("acting", &["machine", "account"]),
    ("plans", &["machine", "agent", "home"]),
    ("tokens", &["key"]),
];

/// The name of `entry` by `key`'s fields; none where it has none of them.
fn name_of(entry: &Value, key: &[&str]) -> Option<String> {
    let fields: Vec<&Value> = key.iter().map(|field| entry.get(*field).unwrap_or(&Value::Null)).collect();
    fields
        .iter()
        .any(|value| !value.is_null())
        .then(|| Value::from(fields.into_iter().cloned().collect::<Vec<_>>()).to_string())
}

/// What of a member the hub tells as it changes (`crate::wire::Down::Presence`):
/// a member that changed in nothing else is not sent again for it.
const PRESENCE: &[&str] = &["online", "since", "through", "reach", "paths"];

/// The field that numbers a view, never itself a change.
const REVISION: &str = "revision";

/// What changed from the view numbered `base` to the one numbered `revision`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewDelta {
    pub base: u64,
    pub revision: u64,
    /// Fields given anew, whole.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set: Map<String, Value>,
    /// Fields the view no longer has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
    /// The long lists, entry by entry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lists: BTreeMap<String, ListDelta>,
}

/// How one long list changed: the entries gone, by name, and those that
/// came or changed, whole. An entry new to the list goes at its end, unless
/// `order` gives the order of every entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ListDelta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub put: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

/// What changed from `old`, numbered `base`, to `new`, numbered `revision`;
/// none when nothing did.
pub fn between(old: &View, new: &View, base: u64, revision: u64) -> Result<Option<ViewDelta>, serde_json::Error> {
    let (Value::Object(old), Value::Object(new)) = (serde_json::to_value(old)?, serde_json::to_value(new)?) else {
        return Err(serde::ser::Error::custom("a view is not an object"));
    };
    let mut delta = ViewDelta { base, revision, set: Map::new(), unset: Vec::new(), lists: BTreeMap::new() };
    for (field, value) in &new {
        if field == REVISION || old.get(field) == Some(value) {
            continue;
        }
        let listed = KEYED.iter().find(|(name, _)| name == field).and_then(|(name, key)| {
            let (Some(Value::Array(before)), Value::Array(after)) = (old.get(field), value) else { return None };
            list(before, after, key, if *name == "members" { PRESENCE } else { &[] })
        });
        match listed {
            Some(list) => {
                delta.lists.insert(field.clone(), list);
            }
            None => {
                delta.set.insert(field.clone(), value.clone());
            }
        }
    }
    delta.unset = old.keys().filter(|field| *field != REVISION && !new.contains_key(*field)).cloned().collect();
    let empty = delta.set.is_empty() && delta.unset.is_empty() && delta.lists.is_empty();
    Ok((!empty).then_some(delta))
}

/// How the list `before` became `after`, entries named by `key`, an entry
/// that changed only in what `told` names left out; none where an entry has
/// no name or shares one, and the list goes whole.
fn list(before: &[Value], after: &[Value], key: &[&str], told: &[&str]) -> Option<ListDelta> {
    let named = |entries: &[Value]| -> Option<Vec<String>> {
        let names: Vec<String> = entries.iter().map(|entry| name_of(entry, key)).collect::<Option<_>>()?;
        let distinct: std::collections::HashSet<&String> = names.iter().collect();
        (distinct.len() == names.len()).then_some(names)
    };
    let (old_names, new_names) = (named(before)?, named(after)?);
    let old: HashMap<&String, &Value> = old_names.iter().zip(before).collect();
    let new: std::collections::HashSet<&String> = new_names.iter().collect();
    let removed: Vec<String> = old_names.iter().filter(|name| !new.contains(name)).cloned().collect();
    let rest = |entry: &Value| -> Value {
        let mut entry = entry.clone();
        if let Value::Object(fields) = &mut entry {
            for field in told {
                fields.remove(*field);
            }
        }
        entry
    };
    let put: Vec<Value> = new_names
        .iter()
        .zip(after)
        .filter(|(name, entry)| {
            old.get(name)
                .is_none_or(|before| if told.is_empty() { before != entry } else { rest(before) != rest(entry) })
        })
        .map(|(_, entry)| entry.clone())
        .collect();
    // What removing and putting leaves: the entries kept in their order,
    // then those new to the list in theirs.
    let left: Vec<&String> = old_names
        .iter()
        .filter(|name| new.contains(name))
        .chain(new_names.iter().filter(|name| !old.contains_key(name)))
        .collect();
    let order = (!left.iter().copied().eq(new_names.iter())).then(|| new_names.clone());
    Some(ListDelta { removed, put, order })
}
