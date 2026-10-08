use std::path::{Path, PathBuf};

/// Accepts only absolute paths of this machine, Git Bash's `/c/…` included
/// (`fastctx_platform::local_path`).
pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = fastctx_platform::local_path(path).map_err(|error| error.to_string())?;
    if !path.is_absolute() {
        return Err(format!("Use an absolute path, not a relative path: {}", path.display()));
    }
    Ok(path)
}
