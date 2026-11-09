//! Sparse confirmation bits avoid retaining a full path for every content bucket.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

// Conservative allocation charges include map nodes, Arc headers and padding.
const GROUP_BYTES: usize = 2048;
const SECOND_BYTES: usize = 256;
type Bits = [u64; 4];

#[derive(Default)]
struct Group {
    root: bool,
    first: Bits,
    second: BTreeMap<u8, Box<Bits>>,
    touched: u64,
}
#[derive(Clone, Copy)]
enum Bucket {
    Root,
    First(u8),
    Second(u8, u8),
}
impl Group {
    fn contains(&self, bucket: Bucket) -> bool {
        match bucket {
            Bucket::Root => self.root,
            Bucket::First(a) => bit(&self.first, a),
            Bucket::Second(a, b) => self.second.get(&a).is_some_and(|bits| bit(bits, b)),
        }
    }
    fn remember(&mut self, bucket: Bucket) {
        match bucket {
            Bucket::Root => self.root = true,
            Bucket::First(a) => set(&mut self.first, a),
            Bucket::Second(a, b) => set(self.second.entry(a).or_default(), b),
        }
    }
}
fn bit(bits: &Bits, index: u8) -> bool {
    bits[index as usize / 64] & (1 << (index % 64)) != 0
}
fn set(bits: &mut Bits, index: u8) {
    bits[index as usize / 64] |= 1 << (index % 64);
}
fn hex(path: &Path) -> Option<u8> {
    let name = path.file_name()?.to_str()?.as_bytes();
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    if name.len() != 2 {
        return None;
    }
    Some(digit(name[0])? * 16 + digit(name[1])?)
}
fn split(path: &Path) -> (&Path, Bucket) {
    if let (Some(b), Some(parent)) = (hex(path), path.parent()) {
        if let (Some(a), Some(root)) = (hex(parent), parent.parent()) {
            return (root, Bucket::Second(a, b));
        }
        return (parent, Bucket::First(b));
    }
    (path, Bucket::Root)
}

#[derive(Default)]
pub(super) struct Confirmed {
    groups: BTreeMap<Arc<Path>, Group>,
    oldest: BTreeSet<(u64, Arc<Path>)>,
    clock: u64,
    pub bytes: usize,
    pub evictions: u64,
}
impl Confirmed {
    pub fn contains(&mut self, path: &Path) -> bool {
        if self.clock == u64::MAX {
            self.evictions = self.evictions.saturating_add(self.groups.len() as u64);
            self.groups.clear();
            self.oldest.clear();
            self.bytes = 0;
            self.clock = 0;
        }
        let (base, bucket) = split(path);
        let Some((key, group)) = self.groups.get_key_value(base) else { return false };
        if !group.contains(bucket) {
            return false;
        }
        let key = key.clone();
        self.oldest.remove(&(group.touched, key.clone()));
        self.clock += 1;
        self.groups.get_mut(base).expect("existing group").touched = self.clock;
        self.oldest.insert((self.clock, key));
        true
    }
    fn additional(&self, base: &Path, bucket: Bucket) -> usize {
        let group = self.groups.get(base);
        let base_bytes = if group.is_some() { 0 } else { GROUP_BYTES + base.as_os_str().len() };
        base_bytes
            + if matches!(bucket, Bucket::Second(a, _) if group.is_none_or(|g| !g.second.contains_key(&a))) {
                SECOND_BYTES
            } else {
                0
            }
    }
    pub fn evict(&mut self) -> bool {
        let Some((_, key)) = self.oldest.pop_first() else { return false };
        let group = self.groups.remove(key.as_ref()).expect("indexed group");
        self.bytes -= GROUP_BYTES + key.as_os_str().len() + group.second.len() * SECOND_BYTES;
        self.evictions = self.evictions.saturating_add(1);
        true
    }
    pub fn remember(&mut self, path: &Path, limit: usize) {
        if self.contains(path) {
            return;
        }
        let (base, bucket) = split(path);
        while self.bytes.saturating_add(self.additional(base, bucket)) > limit {
            if !self.evict() {
                return;
            }
        }
        self.bytes += self.additional(base, bucket);
        let key: Arc<Path> = self.groups.get_key_value(base).map_or_else(|| Arc::from(base), |(key, _)| key.clone());
        let group = self.groups.entry(key.clone()).or_default();
        self.oldest.remove(&(group.touched, key.clone()));
        group.remember(bucket);
        self.clock += 1;
        group.touched = self.clock;
        self.oldest.insert((self.clock, key));
    }
}
