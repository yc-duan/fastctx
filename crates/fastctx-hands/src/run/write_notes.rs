//! Match recorded stdout notes without changing input bytes or terminal parser state.

use fastctx_types::run_facts::{OutputNote, WriteFact, WriteNote};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

fn check_cancelled(cancelled: Option<&AtomicBool>) -> io::Result<()> {
    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "output note projection cancelled"));
    }
    Ok(())
}

pub(super) struct Matches {
    pub ranges: Vec<(Range<u64>, u64)>,
    pub notes: BTreeMap<u64, OutputNote>,
}

fn groups(writes: &[WriteFact]) -> io::Result<Vec<(String, OutputNote)>> {
    let mut operations = BTreeSet::new();
    let mut groups = Vec::new();
    for fact in writes {
        let Some(note) = &fact.note else { continue };
        let operation = fact.id.split_once('-').map_or(fact.id.as_str(), |(operation, _)| operation);
        if !operations.insert(operation) {
            continue;
        }
        let note = match note {
            WriteNote::Text { .. } => note,
            WriteNote::Same { fact: source } => writes
                .iter()
                .find(|fact| &fact.id == source)
                .and_then(|fact| fact.note.as_ref())
                .ok_or_else(|| io::Error::other("recorded stdout note has no byte source"))?,
        };
        let WriteNote::Text { command, text } = note else {
            return Err(io::Error::other("recorded stdout note does not reference its byte source directly"));
        };
        if !text.is_empty() {
            groups.push((text.clone(), OutputNote { command: *command, write: Some(operation.into()) }));
        }
    }
    Ok(groups)
}

/// Append-only, anonymous occurrence index: 16 disk bytes per raw match.
/// Memory follows note metadata and the longest note, never output length.
pub(super) struct Matcher {
    groups: Vec<(String, OutputNote)>,
    patterns: Vec<String>,
    order: Vec<usize>,
    automaton: Option<aho_corasick::AhoCorasick>,
    index: File,
    heads: Vec<u64>,
    tails: Vec<u64>,
    end: u64,
    bytes: u64,
    tail: Vec<u8>,
    prefix: Vec<(Range<u64>, OutputNote)>,
}

impl Matcher {
    pub fn new(writes: &[WriteFact]) -> io::Result<Self> {
        Self::with_prefix(writes, Vec::new())
    }

    fn with_prefix(writes: &[WriteFact], prefix: Vec<(Range<u64>, OutputNote)>) -> io::Result<Self> {
        let groups = groups(writes)?;
        let mut patterns = Vec::new();
        let mut ids = BTreeMap::new();
        let mut order = Vec::new();
        for (text, _) in groups.iter().skip(prefix.len()) {
            let next = patterns.len();
            let id = *ids.entry(text.as_str()).or_insert_with(|| {
                patterns.push(text.clone());
                next
            });
            order.push(id);
        }
        let automaton = if patterns.is_empty() {
            None
        } else {
            Some(aho_corasick::AhoCorasick::new(&patterns).map_err(io::Error::other)?)
        };
        let heads = vec![0; patterns.len()];
        let bytes = prefix.last().map_or(0, |(range, _)| range.end);
        Ok(Self {
            groups,
            patterns,
            order,
            automaton,
            index: tempfile::tempfile()?,
            tails: heads.clone(),
            heads,
            end: 8,
            bytes,
            tail: Vec::new(),
            prefix,
        })
    }

    pub fn same_notes(&self, writes: &[WriteFact]) -> io::Result<bool> {
        Ok(self.groups == groups(writes)?)
    }

    /// A successfully matched leading group cannot move when output is appended.
    /// Preserve that prefix so newly recorded notes search only what follows it.
    pub fn extend_notes(
        &mut self,
        path: &Path,
        bytes: u64,
        writes: &[WriteFact],
        cancelled: Option<&AtomicBool>,
    ) -> io::Result<()> {
        self.append(path, bytes, cancelled)?;
        let selected = self.selected(cancelled)?;
        let mut prefix = Vec::new();
        for (range, tag) in selected.ranges {
            check_cancelled(cancelled)?;
            let note = selected.notes[&tag].clone();
            if self.groups.get(prefix.len()).is_none_or(|(_, expected)| *expected != note) {
                break;
            }
            prefix.push((range, note));
        }
        let incoming = groups(writes)?;
        if incoming.len() < prefix.len() || incoming[..prefix.len()] != self.groups[..prefix.len()] {
            return Err(io::Error::other("recorded stdout note identity or order changed"));
        }
        *self = Self::with_prefix(writes, prefix)?;
        Ok(())
    }

    pub fn append(&mut self, path: &Path, bytes: u64, cancelled: Option<&AtomicBool>) -> io::Result<()> {
        check_cancelled(cancelled)?;
        if bytes < self.bytes {
            return Err(io::Error::other("note capture was truncated"));
        }
        if self.automaton.is_none() {
            self.bytes = bytes;
            return Ok(());
        }
        if bytes == self.bytes {
            return Ok(());
        }
        let overlap = self.patterns.iter().map(String::len).max().expect("patterns") - 1;
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.bytes))?;
        let mut input = file.take(bytes - self.bytes);
        let mut buffer = vec![0; crate::spool::BLOCK + overlap];
        let mut kept = self.tail.len();
        buffer[..kept].copy_from_slice(&self.tail);
        loop {
            check_cancelled(cancelled)?;
            let read = input.read(&mut buffer[kept..])?;
            if read == 0 {
                break;
            }
            let filled = kept + read;
            for found in self.automaton.as_ref().expect("patterns").find_overlapping_iter(&buffer[..filled]) {
                check_cancelled(cancelled)?;
                let start = self.bytes - kept as u64 + found.start() as u64;
                let finish = self.bytes - kept as u64 + found.end() as u64;
                if finish <= self.bytes {
                    continue;
                }
                let id = found.pattern().as_usize();
                if self.tails[id] != 0 {
                    self.index.seek(SeekFrom::Start(self.tails[id] + 8))?;
                    self.index.write_all(&self.end.to_le_bytes())?;
                } else {
                    self.heads[id] = self.end;
                }
                self.index.seek(SeekFrom::Start(self.end))?;
                self.index.write_all(&start.to_le_bytes())?;
                self.index.write_all(&0u64.to_le_bytes())?;
                self.tails[id] = self.end;
                self.end =
                    self.end.checked_add(16).ok_or_else(|| io::Error::other("note occurrence index overflow"))?;
            }
            self.bytes += read as u64;
            kept = overlap.min(filled);
            buffer.copy_within(filled - kept..filled, 0);
        }
        if self.bytes != bytes {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the output prefix is no longer retained"));
        }
        self.tail = buffer[..kept].to_vec();
        Ok(())
    }

    pub fn selected(&mut self, cancelled: Option<&AtomicBool>) -> io::Result<Matches> {
        check_cancelled(cancelled)?;
        let mut result = Matches { ranges: Vec::new(), notes: BTreeMap::new() };
        for (range, note) in &self.prefix {
            check_cancelled(cancelled)?;
            let tag = result.notes.len() as u64 + 1;
            result.ranges.push((range.clone(), tag));
            result.notes.insert(tag, note.clone());
        }
        let mut heads = self.heads.clone();
        let mut cursor = self.prefix.last().map_or(0, |(range, _)| range.end);
        for (operation, &id) in self.order.iter().enumerate() {
            check_cancelled(cancelled)?;
            while heads[id] != 0 {
                check_cancelled(cancelled)?;
                self.index.seek(SeekFrom::Start(heads[id]))?;
                let mut entry = [0; 16];
                self.index.read_exact(&mut entry)?;
                let start = u64::from_le_bytes(entry[..8].try_into().expect("offset"));
                heads[id] = u64::from_le_bytes(entry[8..].try_into().expect("next"));
                if start >= cursor {
                    cursor = start + self.patterns[id].len() as u64;
                    let tag = result.notes.len() as u64 + 1;
                    result.ranges.push((start..cursor, tag));
                    result.notes.insert(tag, self.groups[operation + self.prefix.len()].1.clone());
                    break;
                }
            }
        }
        Ok(result)
    }
}

pub(super) fn tag(ranges: &[(Range<u64>, u64)], byte: u64) -> u64 {
    let at = ranges.partition_point(|(range, _)| range.end <= byte);
    ranges.get(at).filter(|(range, _)| range.contains(&byte)).map_or(0, |(_, tag)| *tag)
}
