//! Where the agents' programs and logins are on a machine, found without
//! running any of them (`world-agent-logins.md`, 世界怎样找到它们).
//!
//! A program is found where its installer or package manager puts it, not
//! only on the search path, and is named by what it was installed with and
//! the versions its package manifest or its file's version information
//! records: an editor extension's or a desktop app's version is that
//! container's, and the command-line program inside has its own, or none
//! known. A login is a directory, recognised by the names of what is in it;
//! nothing in it is read.
//!
//! The places looked in follow each installer's and manager's published
//! defaults and the environment variables that move them, as checked on
//! 2026-10-11. A place this user may not look into is reported, never taken
//! for empty.

mod packages;
mod programs;
pub mod traces;
pub mod walk;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fastctx_types::logins::FoundProgram;

pub use programs::find;

/// What finding the programs needs to know of this user.
pub struct Where<'a> {
    pub user_home: &'a Path,
    /// The user's environment, as the agents' programs get it: where
    /// managers keep their packages, and the search path a terminal uses.
    pub env: &'a [(OsString, OsString)],
    /// The default Codex login, where Codex's own installer keeps its
    /// releases.
    pub codex_home: &'a Path,
    /// The default Claude Code login, where its oldest installer put it.
    pub claude_home: &'a Path,
}

impl Where<'_> {
    fn var(&self, name: &str) -> Option<PathBuf> {
        self.env
            .iter()
            .find(|(key, _)| if cfg!(windows) { key.eq_ignore_ascii_case(name) } else { key == name })
            .map(|(_, value)| PathBuf::from(value))
            .filter(|value| !value.as_os_str().is_empty())
    }

    fn path(&self) -> Vec<PathBuf> {
        self.env
            .iter()
            .find(|(key, _)| if cfg!(windows) { key.eq_ignore_ascii_case("PATH") } else { key == "PATH" })
            .map(|(_, value)| std::env::split_paths(value).filter(|dir| !dir.as_os_str().is_empty()).collect())
            .unwrap_or_default()
    }
}

/// Each agent's programs, and the places that could not be looked into.
#[derive(Debug, Default)]
pub struct Programs {
    pub codex: Vec<FoundProgram>,
    pub claude: Vec<FoundProgram>,
    pub unreadable: Vec<String>,
}

/// Lists a directory's entries, a missing one as empty; any other failure
/// is told to `unreadable` and taken as nothing found there.
fn entries(dir: &Path, unreadable: &mut Vec<String>) -> Vec<std::fs::DirEntry> {
    match std::fs::read_dir(dir) {
        Ok(read) => read
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    unreadable.push(format!("{}: {error}", dir.display()));
                    None
                }
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => Vec::new(),
        Err(error) => {
            unreadable.push(format!("{}: {error}", dir.display()));
            Vec::new()
        }
    }
}

/// A path without Windows' verbatim prefix, which resolving a link adds,
/// as people write it.
pub fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => text.strip_prefix(r"\\?\").map(PathBuf::from).unwrap_or_else(|| path.to_path_buf()),
    }
}

/// Whether two paths are the same file, however each is written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => {
            if cfg!(windows) {
                a.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(&b.to_string_lossy().replace('\\', "/"))
            } else {
                a == b
            }
        }
    }
}
