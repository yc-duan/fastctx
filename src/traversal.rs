//! Shared project traversal for grep and replace.

use globset::GlobSet;
use ignore::types::TypesBuilder;
use ignore::{WalkBuilder, WalkState};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// One deterministic project candidate with its normalized display path.
#[derive(Debug)]
pub(crate) struct ProjectCandidate {
    pub(crate) path: PathBuf,
    pub(crate) display: String,
    pub(crate) modified: SystemTime,
    /// File size at traversal time; consumers may use it to pick an IO strategy.
    pub(crate) file_len: u64,
}

/// Collects files with grep's ignore/hidden/.git semantics and newest-first ordering.
pub(crate) fn collect_project_candidates(
    root: &Path,
    glob: Option<&GlobSet>,
    file_type: Option<&str>,
) -> Result<Vec<ProjectCandidate>, String> {
    let metadata =
        fs::metadata(root).map_err(|error| crate::paths::io_error_message(root, &error))?;
    let type_filter = if let Some(file_type) = file_type {
        let mut builder = TypesBuilder::new();
        builder.add_defaults();
        builder.select(file_type);
        Some(builder.build().map_err(|_| {
            format!(
                "Unknown file type: \"{file_type}\". Run with a glob filter instead, or use a standard type like js, py, rust, go, java."
            )
        })?)
    } else {
        None
    };

    let mut candidates = Vec::new();
    if metadata.is_file() {
        if matches_filters(
            root,
            root.parent().unwrap_or(root),
            glob,
            type_filter.as_ref(),
        ) && let Some(candidate) = candidate_from_path(root)?
        {
            candidates.push(candidate);
        }
    } else {
        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(false)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(false)
            .threads(walk_thread_count())
            .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
        if let Some(types) = type_filter {
            builder.types(types);
        }
        // Directory enumeration dominates large-tree latency, so the walk runs
        // on ignore's parallel walker; determinism comes from the final sort.
        let collected = Mutex::new(Vec::new());
        let failure = Mutex::new(None::<String>);
        builder.build_parallel().run(|| {
            Box::new(|entry| {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        let message = if let Some(io_error) = error.io_error() {
                            let path = project_walk_error_path(&error).unwrap_or(root);
                            crate::paths::io_error_message(path, io_error)
                        } else {
                            format!("Cannot traverse path: {error}")
                        };
                        record_walk_failure(&failure, message);
                        return WalkState::Quit;
                    }
                };
                if !entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file() || file_type.is_symlink())
                {
                    return WalkState::Continue;
                }
                if matches_filters(entry.path(), root, glob, None) {
                    match candidate_from_entry(&entry) {
                        Ok(Some(candidate)) => {
                            collected
                                .lock()
                                .expect("walk sink poisoned")
                                .push(candidate);
                        }
                        Ok(None) => {}
                        Err(message) => {
                            record_walk_failure(&failure, message);
                            return WalkState::Quit;
                        }
                    }
                }
                WalkState::Continue
            })
        });
        if let Some(message) = failure.into_inner().expect("walk failure poisoned") {
            return Err(message);
        }
        candidates = collected.into_inner().expect("walk sink poisoned");
    }
    candidates.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.display.as_bytes().cmp(right.display.as_bytes()))
    });
    Ok(candidates)
}

/// Keeps the first failure; later racing failures are equivalent fail-fast picks.
fn record_walk_failure(failure: &Mutex<Option<String>>, message: String) {
    let mut slot = failure.lock().expect("walk failure poisoned");
    if slot.is_none() {
        *slot = Some(message);
    }
}

pub(crate) fn walk_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
        .min(16)
}

pub(crate) fn project_walk_error_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::Partial(errors) => errors.iter().find_map(project_walk_error_path),
        ignore::Error::WithLineNumber { err, .. } | ignore::Error::WithDepth { err, .. } => {
            project_walk_error_path(err)
        }
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::Io(_)
        | ignore::Error::Glob { .. }
        | ignore::Error::UnrecognizedFileType(_)
        | ignore::Error::InvalidDefinition => None,
    }
}

fn candidate_from_path(path: &Path) -> Result<Option<ProjectCandidate>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::paths::io_error_message(path, &error)),
    };
    candidate_from_metadata(path, &metadata)
}

/// Builds a candidate from walker-provided metadata to avoid a second stat.
///
/// Symlinks still go through the following `fs::metadata` so ordering keys
/// (target mtime) and the is-file check keep their pre-existing semantics.
fn candidate_from_entry(entry: &ignore::DirEntry) -> Result<Option<ProjectCandidate>, String> {
    if entry
        .file_type()
        .is_some_and(|file_type| file_type.is_symlink())
    {
        return candidate_from_path(entry.path());
    }
    match entry.metadata() {
        Ok(metadata) if metadata.is_file() => candidate_from_metadata(entry.path(), &metadata),
        Ok(_) => Ok(None),
        // Fall back to the plain stat so rare metadata failures keep the
        // exact error/skip semantics of the original path-based lookup.
        Err(_) => candidate_from_path(entry.path()),
    }
}

fn candidate_from_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<Option<ProjectCandidate>, String> {
    let modified = metadata
        .modified()
        .map_err(|error| crate::paths::io_error_message(path, &error))?;
    Ok(Some(ProjectCandidate {
        display: crate::paths::display_path(path),
        path: path.to_path_buf(),
        modified,
        file_len: metadata.len(),
    }))
}

fn matches_filters(
    path: &Path,
    root: &Path,
    glob: Option<&GlobSet>,
    types: Option<&ignore::types::Types>,
) -> bool {
    if let Some(types) = types
        && !types.matched(path, false).is_whitelist()
    {
        return false;
    }
    let Some(glob) = glob else {
        return true;
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    glob.is_match(crate::paths::display_path(relative))
}

#[cfg(test)]
mod tests {
    use super::project_walk_error_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn project_walk_errors_keep_the_nested_path_for_diagnostics() {
        let error = ignore::Error::WithDepth {
            depth: 2,
            err: Box::new(ignore::Error::WithPath {
                path: PathBuf::from("nested/private"),
                err: Box::new(ignore::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))),
            }),
        };
        assert_eq!(
            project_walk_error_path(&error),
            Some(Path::new("nested/private"))
        );
    }
}
