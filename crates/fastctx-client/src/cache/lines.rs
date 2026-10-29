//! Immutable diffs and completed output, indexed by their content and projection.

use std::collections::HashMap;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use fastctx_types::MachineName;
use fastctx_types::lines::{Decoding, Direction, Extent, Match, Part, Position};
use fastctx_types::session::{Happened, SessionChange, SessionEvent, What};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::line_rows::Rows;

#[derive(Clone)]
pub(crate) struct Frozen {
    pub key: String,
    pub content: fastctx_types::session::Fingerprint,
    pub body: Option<String>,
    pub policy: Option<String>,
    pub annotation: Option<String>,
}

#[derive(Serialize)]
struct Projection {
    output: fastctx_types::run_facts::OutputRef,
    notes: Vec<fastctx_types::run_facts::OutputNoteMatch>,
    writes: Vec<fastctx_types::run_facts::WriteFact>,
}

impl Projection {
    fn frozen(&self) -> Option<Frozen> {
        let content = self.output.content.as_ref().filter(|content| valid(content))?;
        let policy = self.output.policy.as_ref()?;
        let body = format!("body-{}-{}", content.sha256, super::key(policy));
        let recipe = serde_json::to_string(&(body.as_str(), &self.notes)).expect("output projection serializes");
        let annotation =
            format!("{:x}", Sha256::digest(serde_json::to_vec(&self.notes).expect("output notes serialize")));
        Some(Frozen {
            key: format!("output-{}-{}", content.sha256, super::key(&recipe)),
            content: content.clone(),
            body: Some(body),
            policy: Some(policy.clone()),
            annotation: Some(annotation),
        })
    }

    fn update(&mut self, writes: &[fastctx_types::run_facts::WriteFact]) {
        for write in writes {
            if let Some(previous) = self.writes.iter_mut().find(|previous| previous.id == write.id) {
                *previous = write.clone();
            } else {
                self.writes.push(write.clone());
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Known {
    files: HashMap<(MachineName, String), Frozen>,
    projections: HashMap<(MachineName, String), Projection>,
    jobs: HashMap<(MachineName, fastctx_types::JobId), (MachineName, String)>,
}

impl Known {
    pub fn learn(&mut self, events: &[SessionEvent]) -> Vec<String> {
        let mut keys = Vec::new();
        for event in events {
            if let Happened::Changed { change: SessionChange::RunFactsUpdated { update, .. }, .. } = &event.happened {
                let target = update
                    .output
                    .as_ref()
                    .map(|output| (output.machine.clone(), output.path.clone()))
                    .or_else(|| self.jobs.get(&(update.machine.clone(), update.job.clone())).cloned());
                if let Some(target) = target
                    && let Some(projection) = self.projections.get_mut(&target)
                {
                    if let Some(output) = &update.output {
                        projection.output = output.clone();
                    }
                    projection.update(&update.writes);
                    if let Some(notes) = &update.notes {
                        projection.notes = notes.clone();
                    }
                    if let Some(frozen) = projection.frozen() {
                        keys.extend([frozen.key.clone(), frozen.body.clone().expect("output body")]);
                        self.files.insert(target, frozen);
                    }
                }
                for write in &update.writes {
                    self.learn_diff(write, &mut keys);
                }
                continue;
            }
            let run = match &event.happened {
                Happened::Began { item, .. } | Happened::Finished { item, .. } | Happened::Updated { item, .. } => {
                    let What::Tool(tool) = &item.what else { continue };
                    tool.run.as_deref()
                }
                _ => None,
            };
            let Some(run) = run else { continue };
            if let Some(output) = &run.output {
                let target = (output.machine.clone(), output.path.clone());
                let projection =
                    Projection { output: output.clone(), notes: run.notes.clone(), writes: run.writes.clone() };
                if let Some(frozen) = projection.frozen() {
                    keys.extend([frozen.key.clone(), frozen.body.clone().expect("output body")]);
                    self.files.insert(target.clone(), frozen);
                }
                if let Some(job) = &run.job {
                    self.jobs.insert((run.machine.clone(), job.clone()), target.clone());
                }
                self.projections.insert(target, projection);
            }
            for write in &run.writes {
                self.learn_diff(write, &mut keys);
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    pub fn get(&self, machine: &MachineName, path: &str, decoding: &Decoding) -> Option<Frozen> {
        let frozen = self.files.get(&(machine.clone(), path.to_string()))?;
        match (frozen.key.starts_with("diff-"), decoding) {
            (true, Decoding::File { encoding }) if encoding.as_deref() == Some("utf-8") => Some(frozen.clone()),
            (false, Decoding::Output { .. }) => Some(frozen.clone()),
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.projections.clear();
        self.jobs.clear();
    }

    /// Lets go of what it knows of the files kept as the keys `gone`, no
    /// longer kept, and of the outputs and jobs that led to them.
    pub fn forget(&mut self, gone: &std::collections::HashSet<String>) {
        self.files.retain(|_, frozen| {
            !gone.contains(&frozen.key) && frozen.body.as_ref().is_none_or(|body| !gone.contains(body))
        });
        let files = &self.files;
        self.projections.retain(|target, _| files.contains_key(target));
        let projections = &self.projections;
        self.jobs.retain(|_, target| projections.contains_key(target));
    }

    fn learn_diff(&mut self, write: &fastctx_types::run_facts::WriteFact, keys: &mut Vec<String>) {
        if let Some(diff) = &write.diff
            && let Some(content) = &diff.content
            && valid(content)
        {
            let key = format!("diff-{}", content.sha256);
            self.files.insert(
                (diff.machine.clone(), diff.path.clone()),
                Frozen { key: key.clone(), content: content.clone(), body: None, policy: None, annotation: None },
            );
            keys.push(key);
        }
    }
}

pub(crate) fn valid_key(key: &str) -> bool {
    let hex =
        |text: &str, length| text.len() == length && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    key.strip_prefix("diff-").is_some_and(|key| hex(key, 64))
        || key
            .strip_prefix("output-")
            .or_else(|| key.strip_prefix("body-"))
            .and_then(|key| key.split_once('-'))
            .is_some_and(|(content, projection)| hex(content, 64) && hex(projection, 32))
}

fn valid(content: &fastctx_types::session::Fingerprint) -> bool {
    content.sha256.len() == 64 && content.sha256.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Index {
    pub extent: Extent,
    pub rows: Vec<Row>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Row {
    pub from: u64,
    pub to: u64,
    pub digest: [u8; 32],
}

pub(crate) fn index(root: &Path, key: &str) -> io::Result<Option<Index>> {
    match std::fs::read(root.join("blobs").join(key).join("index.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn read(
    root: &Path,
    key: &str,
    start: u64,
    count: u64,
) -> io::Result<(Vec<String>, Vec<fastctx_types::lines::LineNote>)> {
    let index =
        index(root, key)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the kept lines were cleared"))?;
    let mut rows = Rows::new(start, count);
    let Some(body) = &index.body else {
        visit(root, key, &index, start, count, |part| rows.push(part).map_err(io::Error::other))?;
        return rows.into_lines().map_err(io::Error::other);
    };
    if !body.starts_with("body-") || !valid_key(body) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the kept output names an invalid body"));
    }
    let body_index = index_at(root, body)?;
    if body_index.body.is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept output body names another body"));
    }
    visit(root, body, &body_index, start, count, |part| rows.push(part).map_err(io::Error::other))?;
    let (lines, _) = rows.into_lines().map_err(io::Error::other)?;
    let mut notes: Vec<fastctx_types::lines::LineNote> = Vec::new();
    let mut units = 0u64;
    visit(root, key, &index, start, count, |part| {
        let length = part
            .utf16_length
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a kept annotation has no decoded length"))?;
        if !part.text.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept annotation repeats text"));
        }
        let end = units
            .checked_add(length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "kept annotation length overflow"))?;
        if let Some(note) = part.note.filter(|_| length > 0) {
            if let Some(last) = notes.last_mut()
                && last.line == part.line
                && last.to == units
                && last.note == note
            {
                last.to = end;
            } else {
                notes.push(fastctx_types::lines::LineNote { line: part.line, from: units, to: end, note });
            }
        }
        units = end;
        if part.ended {
            if lines.get((part.line - start) as usize).map(|line| line.encode_utf16().count() as u64) != Some(units) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "kept annotations disagree with the output's length",
                ));
            }
            units = 0;
        }
        Ok(())
    })?;
    Ok((lines, notes))
}

fn index_at(root: &Path, key: &str) -> io::Result<Index> {
    index(root, key)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the kept lines were cleared"))
}

fn visit(
    root: &Path,
    key: &str,
    index: &Index,
    start: u64,
    count: u64,
    mut receive: impl FnMut(Part) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = std::fs::File::open(root.join("blobs").join(key).join("parts.jsonl"))?;
    for (ordinal, row) in index
        .rows
        .iter()
        .enumerate()
        .skip(usize::try_from(start).unwrap_or(usize::MAX))
        .take(usize::try_from(count).unwrap_or(usize::MAX))
    {
        file.seek(SeekFrom::Start(row.from))?;
        let mut bytes = Vec::new();
        let length = row
            .to
            .checked_sub(row.from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a kept line has a reversed byte range"))?;
        (&mut file).take(length).read_to_end(&mut bytes)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if digest != row.digest {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept line's checksum disagrees"));
        }
        for line in bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
            let part: Part = serde_json::from_slice(line).map_err(io::Error::other)?;
            if part.line != ordinal as u64 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept line has another line's position"));
            }
            receive(part)?;
        }
    }
    Ok(())
}

pub(crate) fn find(
    root: &Path,
    key: &str,
    query: &str,
    position: &Position,
    direction: Direction,
) -> io::Result<Option<Match>> {
    if query.is_empty() || query.contains(['\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the literal query must be nonempty and fit within one logical line",
        ));
    }
    let index =
        index(root, key)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the kept lines were cleared"))?;
    let mut line = position.line.min(index.extent.length);
    if direction == Direction::Backward && line == index.extent.length {
        let Some(previous) = line.checked_sub(1) else { return Ok(None) };
        line = previous;
    }
    while line < index.extent.length {
        let (rows, _) = read(root, key, line, 1)?;
        let text = rows
            .first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a kept line is absent from its index"))?;
        let mut found = None;
        for (offset, _) in text.char_indices().filter(|(offset, _)| text[*offset..].starts_with(query)) {
            let offset = text[..offset].encode_utf16().count() as u64;
            let eligible = line != position.line
                || match direction {
                    Direction::Forward => offset >= position.offset.unwrap_or(0),
                    Direction::Backward => offset <= position.offset.unwrap_or(u64::MAX),
                };
            if eligible {
                found = Some(Match { line, offset, length: query.encode_utf16().count() as u64 });
                if direction == Direction::Forward {
                    return Ok(found);
                }
            }
        }
        if found.is_some() {
            return Ok(found);
        }
        match direction {
            Direction::Forward => line += 1,
            Direction::Backward if line > 0 => line -= 1,
            _ => break,
        }
    }
    Ok(None)
}
