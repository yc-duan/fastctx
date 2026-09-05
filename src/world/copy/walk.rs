//! Walks the object a `copy` sends: one file, or a directory tree in sorted order, hashing
//! every regular file as it goes. Symbolic links are reported and not followed; anything that
//! cannot be read is reported and left out. Runs on a blocking thread.

use super::ObjectKind;
use crate::paths::display_path;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One regular file in the manifest.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) index: u64,
    pub(crate) path: PathBuf,
    /// Relative to the copied root, `/`-separated; empty for a single file.
    pub(crate) relative: String,
    pub(crate) size: u64,
    pub(crate) mode: Option<u32>,
    pub(crate) mtime: Option<i64>,
    pub(crate) sha256: String,
}

/// What the walk found, in the order it found it.
#[derive(Debug)]
pub(crate) enum Item {
    Dir(String),
    File(Entry),
    LinkSkipped(String),
    /// Left out of the copy; the source reports it.
    Unreadable {
        path: String,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Totals {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}

/// Walks `root` and hands every item to `emit`, which returns `false` to stop. Returns the
/// number of files and bytes in the manifest.
pub(crate) fn walk(
    root: &Path,
    kind: ObjectKind,
    cancelled: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(Item) -> bool,
) -> Result<Totals, String> {
    let mut totals = Totals::default();
    let mut next_index = 0_u64;
    match kind {
        ObjectKind::File => {
            let metadata = std::fs::metadata(root)
                .map_err(|error| format!("Cannot read {}: {error}", display_path(root)))?;
            let entry = entry_for(root, String::new(), &metadata, next_index)?;
            totals.files = 1;
            totals.bytes = entry.size;
            emit(Item::File(entry));
        }
        ObjectKind::Dir => {
            let mut stack = vec![(root.to_path_buf(), String::new())];
            while let Some((directory, prefix)) = stack.pop() {
                if cancelled() {
                    return Err("the copy was cancelled".to_string());
                }
                let mut children = match std::fs::read_dir(&directory) {
                    Ok(children) => children
                        .filter_map(Result::ok)
                        .map(|child| child.path())
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        if prefix.is_empty() {
                            return Err(format!(
                                "Cannot read {}: {error}",
                                display_path(&directory)
                            ));
                        }
                        if !emit(Item::Unreadable {
                            path: prefix.clone(),
                            reason: error.to_string(),
                        }) {
                            return Ok(totals);
                        }
                        continue;
                    }
                };
                children.sort();
                // Directories are pushed in reverse so the stack pops them in sorted order.
                let mut subdirectories = Vec::new();
                for child in children {
                    let name = child
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let relative = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    let metadata = match std::fs::symlink_metadata(&child) {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            if !emit(Item::Unreadable {
                                path: relative,
                                reason: error.to_string(),
                            }) {
                                return Ok(totals);
                            }
                            continue;
                        }
                    };
                    let item = if metadata.file_type().is_symlink() {
                        Item::LinkSkipped(relative)
                    } else if metadata.is_dir() {
                        subdirectories.push((child.clone(), relative.clone()));
                        Item::Dir(relative)
                    } else if metadata.is_file() {
                        match entry_for(&child, relative.clone(), &metadata, next_index) {
                            Ok(entry) => {
                                next_index += 1;
                                totals.files += 1;
                                totals.bytes += entry.size;
                                Item::File(entry)
                            }
                            Err(reason) => Item::Unreadable {
                                path: relative,
                                reason,
                            },
                        }
                    } else {
                        Item::Unreadable {
                            path: relative,
                            reason: "not a regular file".to_string(),
                        }
                    };
                    if !emit(item) {
                        return Ok(totals);
                    }
                }
                stack.extend(subdirectories.into_iter().rev());
            }
        }
    }
    Ok(totals)
}

fn entry_for(
    path: &Path,
    relative: String,
    metadata: &std::fs::Metadata,
    index: u64,
) -> Result<Entry, String> {
    let sha256 = crate::world::remote_edit::sha256_of(path)?;
    Ok(Entry {
        index,
        path: path.to_path_buf(),
        relative,
        size: metadata.len(),
        mode: unix_mode(metadata),
        mtime: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs() as i64),
        sha256,
    })
}

fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
