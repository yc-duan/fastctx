//! Signed project history revisions. Bodies are read on demand, never indexed.

use crate::ledger::{Entry, File};
use crate::sign::Bytes;
use fastctx_types::history::{Author, Catalog, Document, HistoryRevision, markdown};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    #[serde(flatten)]
    pub revision: HistoryRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<String>,
}

pub fn path(name: &str, version: u64, extension: &str) -> String {
    format!("history/{name}/v{version:020}.{extension}")
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    documents: BTreeMap<String, Vec<Revision>>,
    receipts: BTreeMap<(String, String), u64>,
}

impl Index {
    pub fn latest(&self, name: &str) -> Option<&HistoryRevision> {
        self.documents.get(name).and_then(|versions| versions.last()).map(|stored| &stored.revision)
    }

    pub fn versions(&self, name: &str) -> Vec<HistoryRevision> {
        self.documents.get(name).into_iter().flatten().map(|stored| stored.revision.clone()).collect()
    }

    pub fn get(&self, name: &str, version: Option<u64>) -> Option<&HistoryRevision> {
        match version {
            None => self.latest(name),
            Some(version) => self
                .documents
                .get(name)?
                .get(usize::try_from(version.checked_sub(1)?).ok()?)
                .map(|stored| &stored.revision),
        }
    }

    pub fn submitted(&self, name: &str, submission: &str) -> Option<&HistoryRevision> {
        self.get(name, Some(*self.receipts.get(&(name.into(), submission.into()))?))
    }

    pub fn catalog(&self) -> Catalog {
        let mut catalog = Catalog {
            documents: self
                .documents
                .values()
                .filter_map(|versions| versions.last())
                .map(|stored| stored.revision.clone())
                .collect(),
        };
        catalog.sort();
        catalog
    }

    pub fn check(&self, stored: &Revision) -> Result<(), String> {
        let current = self.latest(&stored.revision.name).map(|revision| revision.version);
        if current.unwrap_or(0).checked_add(1) != Some(stored.revision.version) {
            return Err(fastctx_types::history::conflict(current));
        }
        Ok(())
    }

    pub fn append(&mut self, revision: Revision) {
        if let Some(submission) = &revision.submission {
            self.receipts.insert((revision.revision.name.clone(), submission.clone()), revision.revision.version);
        }
        self.documents.entry(revision.revision.name.clone()).or_default().push(revision);
    }

    pub fn take(&mut self, entry: &Entry) -> Result<(), String> {
        if let Some(revision) = decode(&entry.files)? {
            self.check(&revision)?;
            self.append(revision);
        }
        Ok(())
    }

    pub fn document(&self, name: &str, version: Option<u64>, files: &[File]) -> Result<Document, String> {
        let versions = self.versions(name);
        let revision = match version {
            Some(version) => versions.iter().find(|revision| revision.version == version),
            None => versions.last(),
        }
        .ok_or("the history document or revision does not exist")?
        .clone();
        let text = files
            .iter()
            .find(|file| file.path == path(name, revision.version, "md"))
            .ok_or("the history revision has no Markdown text")?;
        let text = String::from_utf8(text.bytes.0.clone()).map_err(|error| error.to_string())?;
        Ok(Document { revision, text, versions })
    }
}

pub fn has_history(files: &[File]) -> bool {
    files.iter().any(|file| file.path.split('/').next().is_some_and(|part| part.eq_ignore_ascii_case("history")))
}

pub fn decode(files: &[File]) -> Result<Option<Revision>, String> {
    if !has_history(files) {
        return Ok(None);
    }
    if files.len() != 2 || files.iter().any(|file| !file.path.starts_with("history/")) {
        return Err("a history append contains exactly one revision and its Markdown text".into());
    }
    for file in files {
        crate::ledger::check_path(&file.path)?;
    }
    let metadata =
        files.iter().find(|file| file.path.ends_with(".json")).ok_or("the history append has no metadata")?;
    let stored: Revision = serde_json::from_slice(&metadata.bytes.0).map_err(|error| error.to_string())?;
    let revision = &stored.revision;
    markdown::check_name(&revision.name)?;
    if revision.version == 0 || metadata.path != path(&revision.name, revision.version, "json") {
        return Err("the history metadata path does not match its revision".into());
    }
    let text = files
        .iter()
        .find(|file| file.path == path(&revision.name, revision.version, "md"))
        .ok_or("the history append has no matching Markdown text")?;
    let text = std::str::from_utf8(&text.bytes.0).map_err(|error| error.to_string())?;
    let parsed = markdown::parse(&revision.name, text)?;
    if parsed.description != revision.description {
        return Err("the history description differs from its Markdown header".into());
    }
    if stored.submission.as_ref().is_some_and(|id| id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return Err("invalid history submission identity".into());
    }
    Ok(Some(stored))
}

pub fn revision_files(
    name: &str,
    text: &str,
    expected: Option<u64>,
    retired: bool,
    author: Author,
    submission: Option<String>,
) -> Result<(HistoryRevision, Vec<File>), String> {
    let description = markdown::parse_written(name, text)?.description;
    let version = expected.unwrap_or(0).checked_add(1).ok_or("history revision numbers are exhausted")?;
    let revision = HistoryRevision { name: name.into(), description, version, retired, author };
    let stored = Revision { revision: revision.clone(), submission };
    let files = vec![
        File {
            path: path(name, version, "json"),
            bytes: Bytes(serde_json::to_vec(&stored).map_err(|error| error.to_string())?),
        },
        File { path: path(name, version, "md"), bytes: Bytes(text.as_bytes().to_vec()) },
    ];
    decode(&files)?;
    Ok((revision, files))
}
