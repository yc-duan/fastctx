use std::path::{Path, PathBuf};

/// Accepts only absolute native paths, plus Git Bash drive paths on Windows.
pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        let bytes = text.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b'/' {
            return Ok(PathBuf::from(format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &text[2..])));
        }
        if text.starts_with('/') && !text.starts_with("//") {
            return Err(format!(
                "{} is not a Windows absolute path; convert Git Bash mount paths with cygpath -m, then pass that path",
                path.display()
            ));
        }
    }
    if !path.is_absolute() {
        return Err(format!("Use an absolute path, not a relative path: {}", path.display()));
    }
    Ok(path.to_path_buf())
}
