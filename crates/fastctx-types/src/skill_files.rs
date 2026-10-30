//! Atomic, versioned edits of account skill folders. Files are portable relative
//! paths and base64 bytes; a missing body represents an empty directory.

use crate::{
    account_records::Metadata,
    skills::{self, Skill},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One bounded control upload, below the transport's 16 MiB assembled ceiling.
pub const ENCODED_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
}
impl File {
    pub fn text(path: impl Into<String>, text: &str) -> Self {
        Self { path: path.into(), data: Some(STANDARD.encode(text)), executable: false }
    }
    pub fn bytes(&self) -> Result<Option<Vec<u8>>, String> {
        self.data
            .as_ref()
            .map(|data| STANDARD.decode(data).map_err(|error| format!("{}: {error}", self.path)))
            .transpose()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub skill_bytes: u64,
    pub account_bytes: u64,
    pub files: u64,
    pub encoded_bytes: u64,
}
impl Limits {
    pub fn resources(memory: u64, available: u64) -> Self {
        Self {
            skill_bytes: (memory / 128).min(available / 256).min(ENCODED_LIMIT as u64),
            account_bytes: (memory / 8).min(available / 32),
            files: (memory / (256 * 1024)).max(1),
            encoded_bytes: ENCODED_LIMIT as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    Put { file: File },
    Delete { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Limits,
    List,
    Read { name: String },
    ReadFile { name: String, path: String },
    Write { name: String, expected: Option<u64>, files: Vec<File> },
    Edit { name: String, expected: u64, changes: Vec<Change> },
    Delete { name: String, expected: u64 },
}
impl Request {
    pub fn writes(&self) -> bool {
        matches!(self, Self::Write { .. } | Self::Edit { .. } | Self::Delete { .. })
    }
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Read { name }
            | Self::ReadFile { name, .. }
            | Self::Write { name, .. }
            | Self::Edit { name, .. }
            | Self::Delete { name, .. } => Some(name),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.name().is_some_and(|name| !skills::valid_name(name)) {
            return Err(skills::words::NAME.into());
        }
        if serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > ENCODED_LIMIT {
            return Err("the encoded skill upload exceeds 8 MiB".into());
        }
        if let Self::ReadFile { path, .. } = self {
            validate_path(path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub metadata: Option<Metadata>,
    pub skill: Option<Skill>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Reply {
    Limits { limits: Limits },
    Listed { skills: Vec<skills::Listed> },
    Read { snapshot: Box<Snapshot> },
    File { version: u64, file: Option<File> },
    Written { metadata: Metadata },
    Conflict { current: Option<Metadata> },
}

pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 1024 || path.chars().any(|c| c.is_control() || "\\:*?\"<>|".contains(c)) {
        return Err(format!("invalid portable skill path {path:?}"));
    }
    let normalized = crate::names::nfkc(path);
    if normalized != path {
        validate_path(&normalized)?;
    }
    for component in path.split('/') {
        let stem = component.split('.').next().unwrap_or_default().to_ascii_uppercase();
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > 255
            || component.ends_with(['.', ' '])
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(format!("invalid portable skill path {path:?}"));
        }
    }
    Ok(())
}

pub fn validate_files(files: &[File]) -> Result<u64, String> {
    let mut names = BTreeMap::new();
    let mut components = BTreeMap::new();
    let mut bytes = 0u64;
    for file in files {
        validate_path(&file.path)?;
        let mut prefix = String::new();
        for component in file.path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let folded = icu_casemap::CaseMapper::new().fold_string(&crate::names::nfkc(&prefix)).into_owned();
            if components.insert(folded, prefix.clone()).is_some_and(|prior| prior != prefix) {
                return Err("skill directory spellings collide on a case-insensitive computer".into());
            }
        }
        let folded = icu_casemap::CaseMapper::new().fold_string(&crate::names::nfkc(&file.path)).into_owned();
        if names.insert(folded, file.data.is_some()).is_some() {
            return Err("skill paths collide on a case-insensitive computer".into());
        }
        bytes = bytes
            .checked_add(file.bytes()?.map_or(0, |bytes| bytes.len() as u64) + file.path.len() as u64 + 128)
            .ok_or("the skill size overflows")?;
    }
    for path in names.keys() {
        let mut parent = path.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if names.get(prefix) == Some(&true) {
                return Err("a skill file is also used as a directory".into());
            }
            parent = prefix;
        }
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct Header {
    name: String,
    description: String,
}

pub fn from_files(files: Vec<File>) -> Result<Skill, String> {
    validate_files(&files)?;
    let file = files.iter().find(|file| file.path == "SKILL.md").ok_or("a skill folder needs SKILL.md")?;
    let bytes = file.bytes()?.ok_or("SKILL.md must be a file")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "SKILL.md must be UTF-8")?;
    let mut lines = text.strip_prefix('\u{feff}').unwrap_or(text).split_inclusive('\n');
    let first = lines.next().ok_or("SKILL.md is empty")?;
    if first.trim_end() != "---" {
        return Err("SKILL.md needs YAML front matter with name and description".into());
    }
    let rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut end = first.len();
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            let header: Header =
                serde_yaml_ng::from_str(&rest[first.len()..end]).map_err(|error| format!("SKILL.md: {error}"))?;
            let skill = Skill {
                name: header.name,
                description: header.description.trim().into(),
                instructions: rest[end + line.len()..].trim().into(),
                files: Some(files),
            };
            skill.validate()?;
            return Ok(skill);
        }
        end += line.len();
    }
    Err("SKILL.md front matter has no closing delimiter".into())
}

pub fn edit(mut files: Vec<File>, changes: Vec<Change>) -> Result<Vec<File>, String> {
    let mut changed = BTreeSet::new();
    for change in changes {
        let path = match &change {
            Change::Put { file } => &file.path,
            Change::Delete { path } => path,
        };
        validate_path(path)?;
        if !changed.insert(path.clone()) {
            return Err("a file is changed more than once in one edit".into());
        }
        files.retain(|file| file.path != *path);
        if let Change::Put { file } = change {
            files.push(file);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The original front matter, including comments and unknown fields, is kept
/// byte-for-byte when an older editor changes only the instructions.
pub fn legacy_document(original: &File, before: &Skill, incoming: &Skill) -> Result<File, String> {
    let bytes = original.bytes()?.ok_or("SKILL.md must be a file")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "SKILL.md must be UTF-8")?;
    if before.name != incoming.name || before.description != incoming.description {
        if requires_folder_editor(before)? {
            return Err("use a folder-aware editor to change this skill's front matter".into());
        }
        return Ok(incoming.folder()?.remove(0));
    }
    let mut at = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        at += line.len();
        if index > 0 && matches!(line.trim_end(), "---" | "...") {
            let mut replacement = File::text("SKILL.md", &format!("{}\n{}\n", &text[..at], incoming.instructions));
            replacement.executable = original.executable;
            return Ok(replacement);
        }
    }
    Err("SKILL.md front matter has no closing delimiter".into())
}

pub fn requires_folder_editor(skill: &Skill) -> Result<bool, String> {
    let Some(files) = &skill.files else { return Ok(false) };
    if files.len() > 1 {
        return Ok(true);
    }
    let Some(file) = files.iter().find(|file| file.path == "SKILL.md") else { return Ok(true) };
    let bytes = file.bytes()?.ok_or("SKILL.md must be a file")?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "SKILL.md must be UTF-8")?;
    let mut lines = text.trim_start_matches('\u{feff}').split_inclusive('\n');
    let first = lines.next().ok_or("SKILL.md is empty")?;
    let mut header = String::new();
    if first.trim_end() != "---" {
        return Ok(true);
    }
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            break;
        }
        if line.trim_start().starts_with('#') {
            return Ok(true);
        }
        header.push_str(line);
    }
    let header: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&header).map_err(|error| error.to_string())?;
    Ok(header.keys().any(|key| !matches!(key.as_str(), Some("name" | "description"))))
}
