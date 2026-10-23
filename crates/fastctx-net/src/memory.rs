//! The ledger representation of project memory, shared by nodes, devices and hubs.

use std::collections::BTreeMap;

use fastctx_types::memory::{Author, Catalog, Document, Evidence, Kind, Revision, markdown};

use crate::ledger::{Entry, File};
use crate::sign::{Bytes, sha256};

/// Below the multipart envelope limit, including the nested base64 encodings.
pub const CONTENT_LIMIT: usize = 8 * 1024 * 1024;
pub const INLINE_EVIDENCE_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Mutation {
    Revision(Revision),
    Evidence(Evidence),
}

/// Metadata only: neither a catalog nor a hub's index keeps document bodies.
#[derive(Clone, Debug, Default)]
pub struct Index {
    revisions: BTreeMap<String, Vec<Revision>>,
    evidence: BTreeMap<(String, String), Evidence>,
}

impl Index {
    pub fn latest(&self, name: &str) -> Option<&Revision> {
        self.revisions.get(&name.to_lowercase()).and_then(|versions| versions.last())
    }

    pub fn versions(&self, name: &str) -> Vec<Revision> {
        self.revisions.get(&name.to_lowercase()).cloned().unwrap_or_default()
    }

    pub fn catalog(&self) -> Catalog {
        let mut catalog = Catalog::default();
        for revision in self.revisions.values().filter_map(|versions| versions.last()) {
            match revision.kind {
                Kind::Journal => catalog.journals.push(revision.clone()),
                Kind::Spec => catalog.specs.push(revision.clone()),
            }
        }
        catalog.journals.sort_by(|a, b| (b.author.stamp.at, &b.name).cmp(&(a.author.stamp.at, &a.name)));
        catalog
    }

    pub fn evidence(&self, journal: &str) -> Vec<Evidence> {
        self.evidence.values().filter(|item| item.journal == journal).cloned().collect()
    }

    /// Checks a proposed append against the committed state, without changing it.
    pub fn check(&self, files: &[File]) -> Result<Option<Mutation>, String> {
        let Some(mutation) = decode(files)? else { return Ok(None) };
        self.check_mutation(&mutation)?;
        Ok(Some(mutation))
    }

    /// Checks only metadata; decoding and hashing happen before taking store locks.
    pub fn check_mutation(&self, mutation: &Mutation) -> Result<(), String> {
        match mutation {
            Mutation::Revision(revision) => {
                let current = self.latest(&revision.name);
                if current.is_some_and(|old| old.name != revision.name || old.kind != revision.kind) {
                    return Err(fastctx_types::memory::words::NAME_TAKEN.into());
                }
                if revision.kind == Kind::Journal && current.is_some() {
                    return Err(fastctx_types::memory::words::IMMUTABLE_JOURNAL.into());
                }
                let expected = current.map_or(0, |old| old.version);
                if expected.checked_add(1) != Some(revision.version) {
                    return Err(fastctx_types::memory::words::conflict(expected));
                }
            }
            Mutation::Evidence(evidence) => {
                if !self
                    .latest(&evidence.journal)
                    .is_some_and(|head| head.kind == Kind::Journal && head.name == evidence.journal)
                {
                    return Err(fastctx_types::memory::words::EVIDENCE_JOURNAL.into());
                }
                if self.evidence.keys().any(|(journal, name)| {
                    journal == &evidence.journal.to_lowercase() && crate::ledger::conflicts(name, &evidence.name)
                }) {
                    return Err(fastctx_types::memory::words::IMMUTABLE_EVIDENCE.into());
                }
            }
        }
        Ok(())
    }

    pub fn append(&mut self, mutation: Mutation) {
        match mutation {
            Mutation::Revision(revision) => {
                self.revisions.entry(revision.name.to_lowercase()).or_default().push(revision)
            }
            Mutation::Evidence(evidence) => {
                self.evidence.insert((evidence.journal.to_lowercase(), evidence.name.to_lowercase()), evidence);
            }
        }
    }

    pub fn take(&mut self, entry: &Entry) -> Result<(), String> {
        if let Some(mutation) = self.check(&entry.files)? {
            self.append(mutation);
        }
        Ok(())
    }

    pub fn document(&self, name: &str, version: Option<u32>, files: &[File]) -> Result<Document, String> {
        let versions = self.versions(name);
        let revision = match version {
            Some(version) => versions.iter().find(|revision| revision.version == version),
            None => versions.last(),
        }
        .ok_or_else(|| fastctx_types::memory::words::missing(name))?
        .clone();
        let file = files
            .iter()
            .find(|file| file.path == revision.path("md"))
            .ok_or(fastctx_types::memory::words::MISSING_DOCUMENT_TEXT)?;
        let text = String::from_utf8(file.bytes.0.clone()).map_err(|error| error.to_string())?;
        Ok(Document { revision, text, versions, evidence: self.evidence(name) })
    }
}

pub fn has_memory(files: &[File]) -> bool {
    files.iter().any(|file| file.path.split('/').next().is_some_and(|part| part.eq_ignore_ascii_case("memory")))
}

/// A memory append contains exactly one revision or one evidence file.
pub fn decode(files: &[File]) -> Result<Option<Mutation>, String> {
    if !has_memory(files) {
        return Ok(None);
    }
    if files.is_empty() || files.len() > 2 || files.iter().any(|file| !file.path.starts_with("memory/")) {
        return Err(fastctx_types::memory::words::APPEND_SHAPE.into());
    }
    for file in files {
        crate::ledger::check_path(&file.path)?;
    }
    // `memory/<name>/vNNNNNN.json` is a revision, `memory/<journal>/evidence/<file>.json`
    // evidence and `memory/<journal>/files/<file>` its bytes: told apart by position,
    // so a document may itself be called `files` or `evidence`.
    let folder = |path: &str| path.split('/').nth(2).filter(|_| path.split('/').count() >= 4).map(str::to_owned);
    let metadata = files
        .iter()
        .find(|file| file.path.ends_with(".json") && folder(&file.path).as_deref() != Some("files"))
        .ok_or(fastctx_types::memory::words::MISSING_METADATA)?;
    if folder(&metadata.path).as_deref() == Some("evidence") {
        let evidence: Evidence = serde_json::from_slice(&metadata.bytes.0).map_err(|error| error.to_string())?;
        markdown::check_name(&evidence.journal)?;
        crate::ledger::check_path(&evidence.name)?;
        if evidence.sha256.len() != 64
            || !evidence.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(fastctx_types::memory::words::EVIDENCE_DIGEST.into());
        }
        if metadata.path != evidence.path() {
            return Err(fastctx_types::memory::words::EVIDENCE_PATH.into());
        }
        if let Some(location) = &evidence.location {
            if files.len() != 1 || location.path.is_empty() {
                return Err(fastctx_types::memory::words::EXTERNAL_EVIDENCE.into());
            }
        } else {
            let content = files
                .iter()
                .find(|file| file.path == evidence.content_path())
                .ok_or(fastctx_types::memory::words::MISSING_EVIDENCE_BYTES)?;
            if content.bytes.0.len() > INLINE_EVIDENCE_LIMIT
                || content.bytes.0.len() as u64 != evidence.bytes
                || digest(&content.bytes.0) != evidence.sha256
            {
                return Err(fastctx_types::memory::words::EVIDENCE_MISMATCH.into());
            }
        }
        return Ok(Some(Mutation::Evidence(evidence)));
    }
    let revision: Revision = serde_json::from_slice(&metadata.bytes.0).map_err(|error| error.to_string())?;
    markdown::check_name(&revision.name)?;
    if revision.version == 0 || metadata.path != revision.path("json") || files.len() != 2 {
        return Err(fastctx_types::memory::words::REVISION_PATH.into());
    }
    let content = files
        .iter()
        .find(|file| file.path == revision.path("md"))
        .ok_or(fastctx_types::memory::words::MISSING_MARKDOWN)?;
    if content.bytes.0.len() > CONTENT_LIMIT {
        return Err(fastctx_types::memory::words::DOCUMENT_TOO_LARGE.into());
    }
    let text = std::str::from_utf8(&content.bytes.0).map_err(fastctx_types::memory::words::utf8)?;
    let (description, stamped) = markdown::render(&revision.name, revision.kind, text, &revision.author)?;
    if description != revision.description || stamped != text {
        return Err(fastctx_types::memory::words::HEADER_ATTRIBUTION.into());
    }
    Ok(Some(Mutation::Revision(revision)))
}

pub fn revision_files(
    kind: Kind,
    name: &str,
    text: &str,
    expected: Option<u32>,
    author: Author,
) -> Result<(Revision, Vec<File>), String> {
    if kind == Kind::Journal && expected.is_some() {
        return Err(fastctx_types::memory::words::IMMUTABLE_JOURNAL.into());
    }
    let version = expected.unwrap_or(0).checked_add(1).ok_or(fastctx_types::memory::words::VERSION_EXHAUSTED)?;
    let (description, text) = markdown::render(name, kind, text, &author)?;
    let revision = Revision { kind, name: name.into(), description, version, author };
    let files = vec![
        File {
            path: revision.path("json"),
            bytes: Bytes(serde_json::to_vec(&revision).map_err(|error| error.to_string())?),
        },
        File { path: revision.path("md"), bytes: Bytes(text.into_bytes()) },
    ];
    decode(&files)?;
    Ok((revision, files))
}

pub fn evidence_files(evidence: Evidence, bytes: Option<Vec<u8>>) -> Result<Vec<File>, String> {
    let mut files = vec![File {
        path: evidence.path(),
        bytes: Bytes(serde_json::to_vec(&evidence).map_err(|error| error.to_string())?),
    }];
    if let Some(bytes) = bytes {
        files.push(File { path: evidence.content_path(), bytes: Bytes(bytes) });
    }
    decode(&files)?;
    Ok(files)
}

pub fn digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
