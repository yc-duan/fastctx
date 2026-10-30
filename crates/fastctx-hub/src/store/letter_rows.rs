//! Dirty letter records, retained until a confirmed commit. A newer snapshot
//! includes every older uncommitted change, including deletions, so the
//! revision fence can discard late snapshots without dropping any row.
use super::Letters;
use fastctx_types::MachineName;
use std::{collections::BTreeMap, ops::Deref};

pub struct LetterRows {
    entries: BTreeMap<MachineName, Letters>,
    dirty: BTreeMap<String, Dirty>,
    revision: u64,
}
struct Dirty {
    name: MachineName,
    revision: u64,
}

pub(crate) struct Snapshot {
    pub(crate) revision: u64,
    entries: Vec<(MachineName, Option<Letters>)>,
}
impl LetterRows {
    pub(super) fn new(entries: BTreeMap<MachineName, Letters>, changed: bool) -> Self {
        let dirty = if changed {
            entries.keys().map(|key| (key.to_string(), Dirty { name: key.clone(), revision: 1 })).collect()
        } else {
            BTreeMap::new()
        };
        Self { entries, dirty, revision: u64::from(changed) }
    }
    fn touch(&mut self, name: &MachineName) {
        self.revision = self.revision.checked_add(1).expect("letter revision exhausted");
        self.dirty.insert(name.to_string(), Dirty { name: name.clone(), revision: self.revision });
    }
    pub fn get_mut(&mut self, name: &MachineName) -> Option<&mut Letters> {
        let name = self.entries.get_key_value(name)?.0.clone();
        self.touch(&name);
        self.entries.get_mut(&name)
    }
    pub fn insert(&mut self, name: MachineName, value: Letters) -> Option<Letters> {
        let stored = self.entries.get_key_value(&name).map_or_else(|| name.clone(), |(key, _)| key.clone());
        self.touch(&stored);
        self.entries.insert(name, value)
    }
    pub fn remove(&mut self, name: &MachineName) -> Option<Letters> {
        let name = self.entries.get_key_value(name)?.0.clone();
        self.touch(&name);
        self.entries.remove(&name)
    }
    pub(crate) fn snapshot(&mut self, written: u64) -> Snapshot {
        self.dirty.retain(|_, dirty| dirty.revision > written);
        Snapshot {
            revision: self.revision,
            // MachineName compares folded names; SQL keys retain their spelling.
            // A case-only rename deletes the old key and inserts the new key.
            entries: self
                .dirty
                .iter()
                .map(|(spelling, dirty)| {
                    let value = self
                        .entries
                        .get_key_value(&dirty.name)
                        .filter(|(name, _)| name.as_str() == spelling)
                        .map(|(_, value)| value.clone());
                    (dirty.name.clone(), value)
                })
                .collect(),
        }
    }
}
impl Deref for LetterRows {
    type Target = BTreeMap<MachineName, Letters>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl<'a> IntoIterator for &'a LetterRows {
    type Item = (&'a MachineName, &'a Letters);
    type IntoIter = std::collections::btree_map::Iter<'a, MachineName, Letters>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
impl Snapshot {
    pub(crate) fn changes(&self) -> std::io::Result<super::rows::Changes> {
        let mut changes = super::rows::Changes::default();
        for (name, value) in &self.entries {
            let key = name.to_string().into();
            match value {
                Some(value) => changes.put.push(super::rows::Put {
                    kind: "letters",
                    key,
                    position: 0,
                    body: super::rows::body(value)?,
                }),
                None => changes.deleted.push(("letters", key)),
            }
        }
        Ok(changes)
    }
}
