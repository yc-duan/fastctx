//! A change put in the view it starts from, field by field. The long lists
//! go entry by entry as the typed entries they are, so putting a change in
//! costs what the change holds, not what the whole view holds; the few
//! other fields go through their JSON, as the hub gave them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fastctx_types::accounts::TokenSeen;
use serde::Deserialize as _;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{KEYED, ListDelta, ViewDelta, name_of};
use crate::directory::{Account, ActsFor, Assist, Directory, LoginPlan, Member, Project, Share, View};

/// `view` changed as `delta` says, and the names of the members put in it:
/// what came new from the hub, to be read by this machine's clock.
pub fn apply(view: &View, delta: &ViewDelta) -> Result<(View, Vec<String>), String> {
    let keyed = |field: &str| KEYED.iter().any(|(name, _)| *name == field);
    let mut view = view.clone();
    let mut lists = Lists { directory: std::mem::take(&mut view.directory), tokens: std::mem::take(&mut view.tokens) };
    if delta.set.keys().chain(&delta.unset).any(|field| !keyed(field)) {
        let Value::Object(mut object) = serde_json::to_value(&view).map_err(|error| error.to_string())? else {
            return Err("a view is not an object".into());
        };
        for field in delta.unset.iter().filter(|field| !keyed(field)) {
            object.remove(field);
        }
        for (field, value) in delta.set.iter().filter(|(field, _)| !keyed(field)) {
            object.insert(field.clone(), value.clone());
        }
        view = serde_json::from_value(Value::Object(object)).map_err(|error| error.to_string())?;
    }
    for field in delta.unset.iter().filter(|field| keyed(field)) {
        lists.clear(field);
    }
    for (field, value) in delta.set.iter().filter(|(field, _)| keyed(field)) {
        lists.replace(field, value)?;
    }
    let mut members = Vec::new();
    for (field, change) in &delta.lists {
        if field == "members" {
            members.extend(
                change
                    .put
                    .iter()
                    .map(|entry| entry.get("name").and_then(Value::as_str).unwrap_or_default().to_string()),
            );
        }
        lists.change(field, change)?;
    }
    view.directory = lists.directory;
    view.tokens = lists.tokens;
    view.revision = delta.revision;
    Ok((view, members))
}

/// The lists a view's changes go entry by entry in, taken out of it.
struct Lists {
    directory: Directory,
    tokens: Vec<TokenSeen>,
}

impl Lists {
    fn change(&mut self, field: &str, change: &ListDelta) -> Result<(), String> {
        let directory = &mut self.directory;
        match field {
            "members" => put(&mut directory.members, field, change),
            "accounts" => put(&mut directory.accounts, field, change),
            "projects" => put(&mut directory.projects, field, change),
            "shares" => put(&mut directory.shares, field, change),
            "lends" => put(&mut directory.assists, field, change),
            "acting" => put(&mut directory.acting, field, change),
            "plans" => put(&mut directory.plans, field, change),
            "tokens" => put(&mut self.tokens, field, change),
            _ => Err(format!("{field} is not a list sent entry by entry")),
        }
    }

    /// The list `field` given whole.
    fn replace(&mut self, field: &str, value: &Value) -> Result<(), String> {
        fn read<T: DeserializeOwned>(field: &str, value: &Value) -> Result<T, String> {
            T::deserialize(value).map_err(|error| format!("{field}: {error}"))
        }
        let directory = &mut self.directory;
        match field {
            "members" => directory.members = read(field, value)?,
            "accounts" => directory.accounts = read(field, value)?,
            "projects" => directory.projects = read(field, value)?,
            "shares" => directory.shares = read(field, value)?,
            "lends" => directory.assists = read(field, value)?,
            "acting" => directory.acting = read(field, value)?,
            "plans" => directory.plans = read(field, value)?,
            "tokens" => self.tokens = read(field, value)?,
            _ => return Err(format!("{field} is not a list sent entry by entry")),
        }
        Ok(())
    }

    fn clear(&mut self, field: &str) {
        let directory = &mut self.directory;
        match field {
            "members" => directory.members.clear(),
            "accounts" => directory.accounts.clear(),
            "projects" => directory.projects.clear(),
            "shares" => directory.shares.clear(),
            "lends" => directory.assists.clear(),
            "acting" => directory.acting.clear(),
            "plans" => directory.plans.clear(),
            "tokens" => self.tokens.clear(),
            _ => {}
        }
    }
}

/// Puts `change` in the list `entries` of `field`: the entries gone taken
/// out, those that came or changed put in place or at the end, and the
/// order given, if it is.
fn put<E: Entry>(entries: &mut Vec<E>, field: &str, change: &ListDelta) -> Result<(), String> {
    let key = KEYED.iter().find(|(name, _)| *name == field).map(|(_, key)| *key).unwrap_or_default();
    let named = |entry: &E| entry.name().map_err(|error| format!("an entry of {field}: {error}"));
    let gone: HashSet<&String> = change.removed.iter().collect();
    let mut kept = Vec::with_capacity(entries.len());
    let mut at: HashMap<String, usize> = HashMap::with_capacity(entries.len());
    for entry in entries.drain(..) {
        let name = named(&entry)?;
        if !gone.contains(&name) {
            at.insert(name, kept.len());
            kept.push(entry);
        }
    }
    for value in &change.put {
        let name = name_of(value, key).ok_or_else(|| format!("an entry of {field} has no name"))?;
        let entry = E::read(value).map_err(|error| format!("an entry of {field}: {error}"))?;
        debug_assert_eq!(named(&entry).ok().as_ref(), Some(&name), "an entry of {field} is named as the hub names it");
        match at.get(&name) {
            Some(&index) => kept[index] = entry,
            None => {
                at.insert(name, kept.len());
                kept.push(entry);
            }
        }
    }
    if let Some(order) = &change.order {
        if order.len() != kept.len() {
            return Err(format!("the order of {field} names {} entries, not {}", order.len(), kept.len()));
        }
        let mut slots: Vec<Option<E>> = kept.into_iter().map(Some).collect();
        kept = order
            .iter()
            .map(|name| {
                at.get(name).and_then(|&index| slots[index].take()).ok_or_else(|| format!("{field} has no {name}"))
            })
            .collect::<Result<_, _>>()?;
    }
    *entries = kept;
    Ok(())
}

/// An entry of a list sent entry by entry.
trait Entry: Sized {
    /// Its name as the hub gives it (`name_of`): the values of its key's
    /// fields (`KEYED`), in order, as a JSON array.
    fn name(&self) -> serde_json::Result<String>;
    fn read(value: &Value) -> serde_json::Result<Self>;
}

/// The fields of an entry type that name an entry, as `KEYED` lists them.
trait Keyed: DeserializeOwned {
    fn name(&self) -> serde_json::Result<String>;
}

impl<T: Keyed> Entry for Arc<T> {
    fn name(&self) -> serde_json::Result<String> {
        T::name(self)
    }

    fn read(value: &Value) -> serde_json::Result<Self> {
        T::deserialize(value).map(Arc::new)
    }
}

impl Entry for TokenSeen {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.key,))
    }

    fn read(value: &Value) -> serde_json::Result<Self> {
        Self::deserialize(value)
    }
}

impl Keyed for Member {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.name,))
    }
}

impl Keyed for Account {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.name,))
    }
}

impl Keyed for Project {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.town,))
    }
}

impl Keyed for Share {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.machine, &self.account))
    }
}

impl Keyed for Assist {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.machine, &self.account, &self.agent, &self.home))
    }
}

impl Keyed for ActsFor {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.machine, &self.account))
    }
}

impl Keyed for LoginPlan {
    fn name(&self) -> serde_json::Result<String> {
        serde_json::to_string(&(&self.machine, &self.agent, &self.home))
    }
}
