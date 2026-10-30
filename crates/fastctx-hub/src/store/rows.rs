//! What the hub keeps one row an entry in its database (`super::shelf`):
//! what changed since the rows were last written, so that keeping a change
//! writes the rows of what it changed and no others.
//!
//! A list keeps its order: each row has a position, an entry changed in
//! place keeps it, one added or moved to the end takes one after the last,
//! and one taken out leaves a gap. The directory's entries are shared by
//! its copies (`Arc`), so an entry that did not change is the very one the
//! rows were written from, and telling it apart costs a pointer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::sync::Arc;

use serde::Serialize;

/// Row changes, kept in one transaction.
#[derive(Default)]
pub(crate) struct Changes {
    /// Kinds whose rows are all deleted first, to be written whole.
    pub(crate) cleared: Vec<&'static str>,
    pub(crate) put: Vec<Put>,
    /// Rows that hold what they held, at a new position.
    pub(crate) moved: Vec<(&'static str, Arc<str>, i64)>,
    pub(crate) deleted: Vec<(&'static str, Arc<str>)>,
    pub(crate) meta: Vec<(&'static str, Vec<u8>)>,
    /// Machine log records, new.
    pub(crate) log: Vec<crate::machine_log::Row>,
    /// Whether the machine log is written whole: what it held is deleted first.
    pub(crate) log_cleared: bool,
    pub(crate) log_after: Option<u64>,
    /// Machine log records kept before this moment, in seconds since 1970,
    /// are deleted.
    pub(crate) expired: Option<jiff::Timestamp>,
}

pub(crate) struct Put {
    pub kind: &'static str,
    pub key: Arc<str>,
    pub position: i64,
    pub body: Vec<u8>,
}

impl Changes {
    pub(crate) fn is_empty(&self) -> bool {
        self.cleared.is_empty()
            && self.put.is_empty()
            && self.moved.is_empty()
            && self.deleted.is_empty()
            && self.meta.is_empty()
            && self.log.is_empty()
            && !self.log_cleared
            && self.log_after.is_none()
            && self.expired.is_none()
    }

    /// About how many bytes it writes.
    pub(crate) fn bytes(&self) -> usize {
        self.put.iter().map(|put| put.key.len() + put.body.len() + 24).sum::<usize>()
            + self.moved.iter().map(|(_, key, _)| key.len() + 24).sum::<usize>()
            + self.deleted.iter().map(|(_, key)| key.len() + 16).sum::<usize>()
            + self.meta.iter().map(|(_, value)| value.len() + 16).sum::<usize>()
            + self.log.iter().map(crate::machine_log::Row::bytes).sum::<usize>()
    }

    pub(crate) fn meta(&mut self, name: &'static str, value: Vec<u8>) {
        self.meta.push((name, value));
    }
}

pub(crate) fn body(value: &impl Serialize) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(io::Error::other)
}

/// The key of a row made of several fields: their values as JSON.
pub(crate) fn composite(value: &impl Serialize) -> String {
    serde_json::to_value(value).expect("keys are plain values").to_string()
}

#[derive(Clone)]
struct Written<T> {
    entry: Arc<T>,
    key: Arc<str>,
    position: i64,
}

/// A list kept one row an entry, in its order, and what its rows hold.
pub(crate) struct Listed<T> {
    kind: &'static str,
    written: Vec<Written<T>>,
    keys: HashSet<Arc<str>>,
    next: i64,
    /// What its rows hold is not known: the next writing writes it whole.
    whole: bool,
}

/// What a list's rows hold once its changes are kept (`Listed::kept`).
pub(crate) struct ListedNext<T> {
    written: Vec<Written<T>>,
    added: Vec<Arc<str>>,
    removed: Vec<Arc<str>>,
    next: i64,
    whole: bool,
}

impl<T: Serialize> Listed<T> {
    /// A list as its rows were read: `rows` the key and position of each
    /// of `entries`, in order.
    pub(crate) fn read(kind: &'static str, rows: Vec<(String, i64)>, entries: &[Arc<T>]) -> Self {
        debug_assert_eq!(rows.len(), entries.len());
        let next = rows.last().map_or(0, |(_, position)| position + 1);
        let written: Vec<Written<T>> = rows
            .into_iter()
            .zip(entries)
            .map(|((key, position), entry)| Written { entry: entry.clone(), key: key.into(), position })
            .collect();
        let keys = written.iter().map(|written| written.key.clone()).collect();
        Self { kind, written, keys, next, whole: false }
    }

    /// The rows to write so they hold `now`, each entry's row named by
    /// `key`.
    pub(crate) fn changes(
        &self,
        now: &[Arc<T>],
        key: impl Fn(&T) -> String,
        changes: &mut Changes,
    ) -> io::Result<ListedNext<T>> {
        if self.whole {
            return whole(self.kind, now, &key, changes);
        }
        let mut puts = Vec::new();
        let mut moved = Vec::new();
        let mut out = Vec::with_capacity(now.len());
        let (mut i, mut j) = (0, 0);
        // Entries in place: the same one, or one changed where it stands.
        while i < self.written.len() && j < now.len() {
            let written = &self.written[i];
            if !Arc::ptr_eq(&written.entry, &now[j]) {
                if *key(&now[j]) != *written.key {
                    break;
                }
                puts.push(Put {
                    kind: self.kind,
                    key: written.key.clone(),
                    position: written.position,
                    body: body(&*now[j])?,
                });
            }
            out.push(Written { entry: now[j].clone(), key: written.key.clone(), position: written.position });
            i += 1;
            j += 1;
        }
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut next = self.next;
        if i < self.written.len() || j < now.len() {
            // The rest, by key: an entry found after the last one placed
            // keeps its place; one found before it moves to the end.
            let mut rest: HashMap<&str, usize> =
                self.written[i..].iter().enumerate().map(|(at, written)| (&*written.key, i + at)).collect();
            let mut last = out.last().map_or(i64::MIN, |written| written.position);
            let mut fresh: HashSet<String> = HashSet::new();
            for entry in &now[j..] {
                let named = key(entry);
                match rest.remove(named.as_str()) {
                    Some(at) => {
                        let written = &self.written[at];
                        let position = if written.position > last {
                            written.position
                        } else {
                            next += 1;
                            next - 1
                        };
                        if !Arc::ptr_eq(&written.entry, entry) {
                            puts.push(Put {
                                kind: self.kind,
                                key: written.key.clone(),
                                position,
                                body: body(&**entry)?,
                            });
                        } else if position != written.position {
                            moved.push((self.kind, written.key.clone(), position));
                        }
                        last = position;
                        out.push(Written { entry: entry.clone(), key: written.key.clone(), position });
                    }
                    // Two entries of one key, which nothing should make: the
                    // list is written whole, each row named apart.
                    None if self.keys.contains(named.as_str()) || fresh.contains(&named) => {
                        return whole(self.kind, now, &key, changes);
                    }
                    None => {
                        let position = next;
                        next += 1;
                        last = position;
                        let named: Arc<str> = named.into();
                        fresh.insert(named.to_string());
                        puts.push(Put { kind: self.kind, key: named.clone(), position, body: body(&**entry)? });
                        added.push(named.clone());
                        out.push(Written { entry: entry.clone(), key: named, position });
                    }
                }
            }
            let mut gone: Vec<usize> = rest.into_values().collect();
            gone.sort_unstable();
            for at in gone {
                let key = self.written[at].key.clone();
                changes.deleted.push((self.kind, key.clone()));
                removed.push(key);
            }
        }
        changes.put.extend(puts);
        changes.moved.extend(moved);
        Ok(ListedNext { written: out, added, removed, next, whole: false })
    }

    /// Its rows hold what `next` says, their changes kept.
    pub(crate) fn kept(&mut self, next: ListedNext<T>) {
        if next.whole {
            self.keys = next.written.iter().map(|written| written.key.clone()).collect();
        } else {
            for key in next.removed {
                self.keys.remove(&key);
            }
            self.keys.extend(next.added);
        }
        self.written = next.written;
        self.next = next.next;
        self.whole = false;
    }

    /// What its rows hold is not known any more: a writing failed.
    pub(crate) fn lost(&mut self) {
        self.whole = true;
    }
}

/// Every row of `kind` anew, holding `now` in order; two entries of one
/// key are named apart.
fn whole<T: Serialize>(
    kind: &'static str,
    now: &[Arc<T>],
    key: &impl Fn(&T) -> String,
    changes: &mut Changes,
) -> io::Result<ListedNext<T>> {
    changes.cleared.push(kind);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut written = Vec::with_capacity(now.len());
    for (position, entry) in now.iter().enumerate() {
        let mut named = key(entry);
        let count = seen.entry(named.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            named = format!("{named}#{count}");
        }
        let named: Arc<str> = named.into();
        let position = i64::try_from(position).map_err(io::Error::other)?;
        changes.put.push(Put { kind, key: named.clone(), position, body: body(&**entry)? });
        written.push(Written { entry: entry.clone(), key: named, position });
    }
    let next = i64::try_from(now.len()).map_err(io::Error::other)?;
    Ok(ListedNext { written, added: Vec::new(), removed: Vec::new(), next, whole: true })
}

/// A map kept one row a key, and what its rows hold.
pub(crate) struct Mapped<K, V> {
    kind: &'static str,
    written: BTreeMap<K, V>,
    whole: bool,
}

/// What a map's rows hold once its changes are kept (`Mapped::kept`).
pub(crate) struct MappedNext<K, V> {
    set: Vec<(K, V)>,
    removed: Vec<K>,
    whole: Option<BTreeMap<K, V>>,
}

impl<K: Ord + Clone, V: Clone + PartialEq + Serialize> Mapped<K, V> {
    /// A map as its rows were read.
    pub(crate) fn read(kind: &'static str, written: BTreeMap<K, V>) -> Self {
        Self { kind, written, whole: false }
    }

    pub(crate) fn changes(
        &self,
        now: &BTreeMap<K, V>,
        key: impl Fn(&K) -> String,
        changes: &mut Changes,
    ) -> io::Result<MappedNext<K, V>> {
        if self.whole {
            changes.cleared.push(self.kind);
            for (name, value) in now {
                changes.put.push(Put { kind: self.kind, key: key(name).into(), position: 0, body: body(value)? });
            }
            return Ok(MappedNext { set: Vec::new(), removed: Vec::new(), whole: Some(now.clone()) });
        }
        let mut set = Vec::new();
        let mut removed = Vec::new();
        let mut written = self.written.iter().peekable();
        let mut current = now.iter().peekable();
        loop {
            match (written.peek(), current.peek()) {
                (None, None) => break,
                (Some((name, _)), None) => {
                    changes.deleted.push((self.kind, key(name).into()));
                    removed.push((*name).clone());
                    written.next();
                }
                (None, Some((name, value))) => {
                    changes.put.push(Put { kind: self.kind, key: key(name).into(), position: 0, body: body(*value)? });
                    set.push(((*name).clone(), (*value).clone()));
                    current.next();
                }
                (Some((was, old)), Some((name, value))) => match was.cmp(name) {
                    std::cmp::Ordering::Less => {
                        changes.deleted.push((self.kind, key(was).into()));
                        removed.push((*was).clone());
                        written.next();
                    }
                    std::cmp::Ordering::Greater => {
                        changes.put.push(Put {
                            kind: self.kind,
                            key: key(name).into(),
                            position: 0,
                            body: body(*value)?,
                        });
                        set.push(((*name).clone(), (*value).clone()));
                        current.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if old != value {
                            changes.put.push(Put {
                                kind: self.kind,
                                key: key(name).into(),
                                position: 0,
                                body: body(*value)?,
                            });
                            set.push(((*name).clone(), (*value).clone()));
                        }
                        written.next();
                        current.next();
                    }
                },
            }
        }
        Ok(MappedNext { set, removed, whole: None })
    }

    /// Changes for a caller that mutated exactly this key. After an uncertain
    /// write, the whole authoritative map still repairs the durable baseline.
    pub(crate) fn change_one(
        &self,
        now: &BTreeMap<K, V>,
        name: &K,
        key: impl Fn(&K) -> String,
        changes: &mut Changes,
    ) -> io::Result<MappedNext<K, V>> {
        if self.whole {
            return self.changes(now, key, changes);
        }
        let mut next = MappedNext { set: Vec::new(), removed: Vec::new(), whole: None };
        if self.written.get(name) != now.get(name) {
            match now.get(name) {
                Some(value) => {
                    changes.put.push(Put { kind: self.kind, key: key(name).into(), position: 0, body: body(value)? });
                    next.set.push((name.clone(), value.clone()));
                }
                None => {
                    changes.deleted.push((self.kind, key(name).into()));
                    next.removed.push(name.clone());
                }
            }
        }
        Ok(next)
    }

    pub(crate) fn kept(&mut self, next: MappedNext<K, V>) {
        if let Some(whole) = next.whole {
            self.written = whole;
        } else {
            for name in next.removed {
                self.written.remove(&name);
            }
            self.written.extend(next.set);
        }
        self.whole = false;
    }

    pub(crate) fn lost(&mut self) {
        self.whole = true;
    }
}
