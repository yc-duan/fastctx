//! Earlier versions kept a copy of a Claude Code login's `settings.json`
//! before and after every `claude plugin` command they ran, in
//! `claude-command-history`, and never removed them. Those settings can carry
//! the login's long-lived token, and fastctx keeps no credential: the image
//! before a command now lives only in memory for the length of the change
//! (`world-run.md`, apply). What an earlier version left is removed wherever
//! an apply, an unapply, the node's start or an uninstall finds it.

use std::fs;
use std::io;
use std::path::Path;

const DIRECTORY: &str = "claude-command-history";

/// The files an earlier version wrote into each `command-<hex>` directory.
const FILES: [&str; 5] = ["command.json", "before.json", "before-metadata.json", "after.json", "after-metadata.json"];

/// Removes the settings copies under `root`. Returns, for each thing that
/// could not go, where it is and why; an entry fastctx did not write is
/// named and left.
pub fn retire(root: &Path) -> Vec<String> {
    let history = root.join(DIRECTORY);
    let mut left = Vec::new();
    match fs::symlink_metadata(&history) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return left,
        Err(error) => return vec![format!("{}: {error}", history.display())],
        Ok(metadata) if crate::disk::is_link(&metadata) || !metadata.is_dir() => {
            return vec![format!("{}: not a directory fastctx made; left", history.display())];
        }
        Ok(_) => {}
    }
    let entries = match fs::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) => return vec![format!("{}: {error}", history.display())],
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                left.push(format!("{}: {error}", history.display()));
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name();
        let command = name
            .to_str()
            .and_then(|name| name.strip_prefix("command-"))
            .is_some_and(|suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let directory =
            fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir() && !crate::disk::is_link(&metadata));
        if !command || !directory {
            left.push(format!("{}: not written by fastctx; left", path.display()));
            continue;
        }
        retire_command(&path, &mut left);
    }
    if left.is_empty()
        && let Err(error) = fs::remove_dir(&history)
        && error.kind() != io::ErrorKind::NotFound
    {
        left.push(format!("{}: {error}", history.display()));
    }
    left
}

fn retire_command(directory: &Path, left: &mut Vec<String>) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            left.push(format!("{}: {error}", directory.display()));
            return;
        }
    };
    let mut kept = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                left.push(format!("{}: {error}", directory.display()));
                kept = true;
                continue;
            }
        };
        let path = entry.path();
        let ours = entry.file_name().to_str().is_some_and(|name| FILES.contains(&name))
            && fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file());
        if !ours {
            left.push(format!("{}: not written by fastctx; left", path.display()));
            kept = true;
            continue;
        }
        if let Err(error) = fs::remove_file(&path) {
            left.push(format!("{}: {error}", path.display()));
            kept = true;
        }
    }
    if !kept && let Err(error) = fs::remove_dir(directory) {
        left.push(format!("{}: {error}", directory.display()));
    }
}
