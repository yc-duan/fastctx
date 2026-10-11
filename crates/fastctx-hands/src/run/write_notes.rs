//! Match recorded stdout notes without changing a byte or the terminal parser's state.

use fastctx_types::run_facts::{OutputNote, WriteFact, WriteNote};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;

pub(super) struct Matches {
    pub ranges: Vec<(Range<u64>, u64)>,
    pub notes: BTreeMap<u64, OutputNote>,
}

/// One pass over the capture, with a disk-linked occurrence index (16 bytes/match).
/// Missing notes leave the forward cursor where the last successful match ended.
pub(super) fn read(path: &Path, bytes: u64, writes: &[WriteFact]) -> io::Result<Matches> {
    let mut operations = BTreeSet::new();
    let mut patterns = Vec::<&str>::new();
    let mut pattern_ids = BTreeMap::new();
    let mut ordered = Vec::new();
    for fact in writes {
        let Some(note) = &fact.note else { continue };
        let operation = fact.id.split_once('-').map_or(fact.id.as_str(), |(operation, _)| operation);
        // A patch emits one note for all its mutations, not one copy per path.
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
        if text.is_empty() {
            continue;
        }
        let next = patterns.len();
        let id = *pattern_ids.entry(text.as_str()).or_insert_with(|| {
            patterns.push(text);
            next
        });
        ordered.push((id, OutputNote { command: *command, write: Some(operation.into()) }));
    }
    let mut result = Matches { ranges: Vec::new(), notes: BTreeMap::new() };
    if patterns.is_empty() {
        return Ok(result);
    }
    let matcher = aho_corasick::AhoCorasick::new(&patterns).map_err(io::Error::other)?;
    let overlap = patterns.iter().map(|pattern| pattern.len()).max().expect("patterns") - 1;
    let mut input = File::open(path)?.take(bytes);
    let mut buffer = vec![0; crate::spool::BLOCK + overlap];
    let mut index = tempfile::tempfile()?;
    let mut heads = vec![0u64; patterns.len()];
    let mut tails = heads.clone();
    let mut end = 8u64;
    let mut offset = 0u64;
    let mut kept = 0;
    loop {
        let read = input.read(&mut buffer[kept..])?;
        if read == 0 {
            break;
        }
        let filled = kept + read;
        for found in matcher.find_overlapping_iter(&buffer[..filled]) {
            let start = offset - kept as u64 + found.start() as u64;
            let finish = offset - kept as u64 + found.end() as u64;
            if finish <= offset {
                continue;
            }
            let id = found.pattern().as_usize();
            if tails[id] != 0 {
                index.seek(SeekFrom::Start(tails[id] + 8))?;
                index.write_all(&end.to_le_bytes())?;
            } else {
                heads[id] = end;
            }
            index.seek(SeekFrom::Start(end))?;
            index.write_all(&start.to_le_bytes())?;
            index.write_all(&0u64.to_le_bytes())?;
            tails[id] = end;
            end = end.checked_add(16).ok_or_else(|| io::Error::other("note occurrence index overflow"))?;
        }
        offset += read as u64;
        kept = overlap.min(filled);
        buffer.copy_within(filled - kept..filled, 0);
    }
    if offset != bytes {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the output prefix is no longer retained"));
    }
    let mut cursor = 0;
    for (id, note) in ordered {
        while heads[id] != 0 {
            index.seek(SeekFrom::Start(heads[id]))?;
            let mut entry = [0; 16];
            index.read_exact(&mut entry)?;
            let start = u64::from_le_bytes(entry[..8].try_into().expect("offset"));
            heads[id] = u64::from_le_bytes(entry[8..].try_into().expect("next"));
            if start >= cursor {
                cursor = start + patterns[id].len() as u64;
                let tag = result.notes.len() as u64 + 1;
                result.ranges.push((start..cursor, tag));
                result.notes.insert(tag, note);
                break;
            }
        }
    }
    Ok(result)
}

pub(super) fn tag(ranges: &[(Range<u64>, u64)], byte: u64) -> u64 {
    let at = ranges.partition_point(|(range, _)| range.end <= byte);
    ranges.get(at).filter(|(range, _)| range.contains(&byte)).map_or(0, |(_, tag)| *tag)
}
