use std::path::{Path, PathBuf};

/// Accepts only absolute paths of this machine, Git Bash's `/c/…` included
/// (`fastctx_platform::local_path`).
pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = fastctx_platform::local_path(path).map_err(|error| error.to_string())?;
    if !path.is_absolute() {
        return Err(format!(
            "{} is a relative path; the World's file commands take only absolute paths, as /… or C:/… on Windows",
            path.display()
        ));
    }
    Ok(path)
}
