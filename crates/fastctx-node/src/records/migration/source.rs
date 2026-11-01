//! A decoded legacy row, or the boundary that proves its original file.
use crate::record_migration::{FileComplete, FileRow};
use std::{io, path::PathBuf};

pub(crate) enum Source {
    Row(LegacyRow),
    File(FileComplete),
}
pub(crate) struct LegacyRow {
    pub key: String,
    pub conversation: Option<String>,
    pub body: Vec<u8>,
    pub modified: jiff::Timestamp,
    pub arrival: Option<Vec<u8>>,
    pub replace: bool,
    pub category: Option<String>,
    pub expires: Option<jiff::Timestamp>,
    pub file: Option<FileRow>,
    pub discard: bool,
    pub content_key: Option<(String, bool)>,
}
impl Source {
    fn row(&mut self) -> &mut LegacyRow {
        match self {
            Self::Row(row) => row,
            Self::File(_) => unreachable!("only decoded rows have record attributes"),
        }
    }
    pub(crate) fn conversation(&self) -> Option<&str> {
        match self {
            Self::Row(row) => row.conversation.as_deref(),
            Self::File(_) => None,
        }
    }
    pub(crate) fn set_file(&mut self, file: FileRow) {
        self.row().file = Some(file);
    }
    pub(crate) fn content_key(mut self, deduplicate: bool) -> Self {
        use sha2::{Digest, Sha256};
        let row = self.row();
        row.content_key = Some((format!("{:x}", Sha256::digest(&row.body)), deduplicate));
        self
    }
    pub(crate) fn modified(&self) -> jiff::Timestamp {
        match self {
            Self::Row(row) => row.modified,
            Self::File(_) => unreachable!("file boundaries have no record time"),
        }
    }
    pub(crate) fn scoped(mut self, conversation: String) -> Self {
        self.row().conversation = Some(conversation);
        self
    }
    pub(crate) fn expiring(mut self, at: jiff::Timestamp) -> Self {
        self.row().expires = Some(at);
        self
    }
    pub(crate) fn ordered_bytes(mut self, order: Vec<u8>) -> Self {
        self.row().arrival = Some(order);
        self
    }
    pub(crate) fn categorized(mut self, category: String) -> Self {
        self.row().category = Some(category);
        self
    }
    pub(crate) fn replacing(mut self) -> Self {
        self.row().replace = true;
        self
    }
    pub(crate) fn ordered(self, at: jiff::Timestamp) -> Self {
        self.ordered_bytes((at.as_nanosecond() ^ i128::MIN).to_be_bytes().to_vec())
    }
    pub(crate) fn skip(path: PathBuf) -> io::Result<Self> {
        let mut source = Self::new(String::new(), path, Vec::new())?;
        source.row().discard = true;
        Ok(source)
    }
    pub(crate) fn new(key: String, path: PathBuf, body: Vec<u8>) -> io::Result<Self> {
        let modified = crate::record_migration::files::modified(&path)?;
        Ok(Self::Row(LegacyRow {
            key,
            conversation: None,
            body,
            modified,
            arrival: None,
            replace: false,
            category: None,
            expires: None,
            file: None,
            discard: false,
            content_key: None,
        }))
    }
}
