//! Byte-preserving batch replacement with optimistic and cross-process write protection.

mod document;
mod locks;
pub(crate) mod private_storage;
mod replace;

use crate::budget::token_budget;
use crate::glob_filter::GlobPatterns;
use crate::model::ToolResponse;
use locks::PathIdentity;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

/// Parameters for deterministic batch replacement across one file or a project tree.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct ReplaceRequest {
    /// The regular expression to replace (Rust regex; escape literal braces).
    pub pattern: String,
    /// Replacement text; $1/${name} reference groups, $$ is a literal $, empty deletes the match.
    pub replacement: String,
    /// File or directory to edit.
    #[schemars(description = crate::model_guidance::local_path_description(
        "File or directory to edit."
    ))]
    pub path: String,
    /// Globs for directory targets, e.g. ["**/*.rs", "!tests/**"]. A leading `!` excludes and always wins; negative-only lists include every other file.
    // Published as a plain string array; see the note on GlobRequest::pattern. (2026-08-17)
    #[schemars(with = "Option<Vec<String>>")]
    pub glob: Option<GlobPatterns>,
    /// Treat pattern as a literal string, not a regex.
    pub literal: Option<bool>,
    /// Case-insensitive matching.
    pub case_insensitive: Option<bool>,
    /// `.` also matches newlines (spanning-line matches); `\n` also matches `\r\n`.
    pub dot_all: Option<bool>,
    /// Refuse to write if the total match count exceeds this guard.
    pub max_replacements: Option<usize>,
    /// Preview matches and counts without writing anything.
    pub dry_run: Option<bool>,
    /// Single-file target only: decode with this WHATWG label.
    pub encoding: Option<String>,
    /// Directory target: fallback encoding for otherwise unresolved files.
    pub fallback_encoding: Option<String>,
}

/// Stateful replacement service sharing in-process locks across concurrent calls.
#[derive(Clone, Debug)]
pub struct ReplaceService {
    path_locks: Arc<PathLocks>,
}

#[derive(Debug, Default)]
struct PathLocks {
    locks: Mutex<BTreeMap<PathIdentity, Weak<Mutex<()>>>>,
}

impl PathLocks {
    fn for_identity(&self, identity: &PathIdentity) -> Arc<Mutex<()>> {
        let key = identity.clone();
        let mut locks = self.locks.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }
}

impl ReplaceService {
    /// Creates a replacement service with shared target-identity locks.
    pub fn new() -> Self {
        Self {
            path_locks: Arc::new(PathLocks::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_locks_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.path_locks, &other.path_locks)
    }

    /// Applies one regex plan across all candidates after a full blast-radius count.
    pub fn replace(&self, request: ReplaceRequest) -> ToolResponse {
        self.replace_with_limit(
            request,
            crate::control::settings::DEFAULT_REPLACE_FILE_LIMIT_MIB as u64,
        )
    }

    pub(crate) fn replace_with_limit(
        &self,
        request: ReplaceRequest,
        max_file_size_mib: u64,
    ) -> ToolResponse {
        replace::replace(self, request, max_file_size_mib)
    }
}

impl Default for ReplaceService {
    fn default() -> Self {
        Self::new()
    }
}

fn edit_token_budget() -> Result<usize, String> {
    token_budget().map_err(|message| format!("{message} Fix the env var and restart the session."))
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use super::{ReplaceRequest, ReplaceService};
    use crate::edit::document::set_before_commit_hook;

    fn response_text(response: &crate::ToolResponse) -> &str {
        match response.content.as_slice() {
            [crate::ToolContent::Text(text)] => text,
            content => panic!("expected one text block, got {content:?}"),
        }
    }

    fn request(path: &std::path::Path, pattern: &str, replacement: &str) -> ReplaceRequest {
        ReplaceRequest {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            path: crate::paths::display_path(path),
            glob: None,
            literal: None,
            case_insensitive: None,
            dot_all: None,
            max_replacements: None,
            dry_run: None,
            encoding: None,
            fallback_encoding: None,
        }
    }

    #[test]
    fn replace_cas_rejects_external_changes() {
        let temp = tempfile::tempdir().unwrap();
        let editor = ReplaceService::new();
        let path = temp.path().join("replace.txt");
        std::fs::write(&path, b"old").unwrap();
        set_before_commit_hook(|path| std::fs::write(path, b"external replace").unwrap());

        let response = editor.replace(request(&path, "old", "new"));

        assert!(!response.is_error);
        assert!(response_text(&response).contains("changed on disk during the edit"));
        assert!(
            response_text(&response).starts_with(
                "=== replace (0 replacements in 0 files; 1 file failed — see report) ==="
            ),
            "{}",
            response_text(&response)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external replace");
    }

    #[test]
    fn candidate_set_is_frozen_before_the_first_replace_write() {
        let temp = tempfile::tempdir().unwrap();
        let editor = ReplaceService::new();
        let root = temp.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"hit\n").unwrap();
        std::fs::write(root.join("b.txt"), b"hit\n").unwrap();
        std::fs::write(root.join(".gitignore"), b"").unwrap();
        let ignore = root.join(".gitignore");
        set_before_commit_hook(move |_| std::fs::write(ignore, b"*.txt\n").unwrap());

        let response = editor.replace(request(&root, "hit", "done"));

        assert!(!response.is_error, "{}", response_text(&response));
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"done\n");
        assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"done\n");
    }

    #[cfg(unix)]
    #[test]
    fn replace_recognizes_symlink_aliases_as_one_target_identity() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let editor = ReplaceService::new();
        let root = temp.path().join("tree");
        std::fs::create_dir(&root).unwrap();
        let target = root.join("target.txt");
        let alias = root.join("alias.txt");
        std::fs::write(&target, b"hit\n").unwrap();
        symlink(&target, &alias).unwrap();

        let response = editor.replace(request(&root, "hit", "done"));

        assert!(!response.is_error, "{}", response_text(&response));
        assert!(
            response_text(&response).starts_with("=== replace (1 replacement in 1 file) ==="),
            "{}",
            response_text(&response)
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"done\n");
        assert!(
            std::fs::symlink_metadata(alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
