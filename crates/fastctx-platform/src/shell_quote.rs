//! Quoting paths in POSIX shell commands shown for a person to run.

use std::path::Path;

/// One shell word, including spaces, substitutions and embedded quotes.
pub fn path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
