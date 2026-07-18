//! Recognition and cleanup of binary replacement artifacts owned by FastCtx.

use std::fs;
use std::path::{Path, PathBuf};

/// Lists regular-file siblings that match either FastCtx replacement naming scheme.
pub(crate) fn stale_binary_siblings(target: &Path) -> Result<Vec<PathBuf>, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "The FastCtx binary path has no parent directory".to_string())?;
    let target_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The FastCtx binary filename is not valid Unicode".to_string())?;
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Cannot inspect the FastCtx binary directory {}: {error}",
                crate::paths::display_path(parent)
            ));
        }
    };
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect an entry in {}: {error}",
                crate::paths::display_path(parent)
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "Cannot inspect {}: {error}",
                crate::paths::display_path(&entry.path())
            )
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if file_type.is_file() && is_stale_binary_name(target_name, &name) {
            stale.push(entry.path());
        }
    }
    stale.sort();
    Ok(stale)
}

/// Opportunistically removes artifacts that are no longer mapped by a running process.
pub(crate) fn cleanup_stale_binary_siblings(target: &Path) {
    let Ok(paths) = stale_binary_siblings(target) else {
        return;
    };
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn is_stale_binary_name(target_name: &str, candidate: &str) -> bool {
    let rename_old_prefix = format!(".{target_name}.fastctx-old-");
    if let Some(suffix) = candidate.strip_prefix(&rename_old_prefix)
        && let Some((pid, sequence)) = suffix.split_once('.')
        && !pid.is_empty()
        && !sequence.is_empty()
        && !sequence.contains('.')
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
    {
        return true;
    }

    let candidate_lower = candidate.to_ascii_lowercase();
    let replace_file_prefix = format!("{}~rf", target_name.to_ascii_lowercase());
    candidate_lower.starts_with(&replace_file_prefix) && candidate_lower.ends_with(".tmp")
}

#[cfg(test)]
mod tests {
    use super::{is_stale_binary_name, stale_binary_siblings};

    #[test]
    fn stale_name_recognizer_accepts_both_owned_schemes_and_rejects_lookalikes() {
        assert!(is_stale_binary_name(
            "fastctx.exe",
            ".fastctx.exe.fastctx-old-12.0"
        ));
        assert!(is_stale_binary_name(
            "fastctx.exe",
            "fastctx.exe~RF1a2B.TMP"
        ));
        assert!(!is_stale_binary_name(
            "fastctx.exe",
            ".other.exe.fastctx-old-12.0"
        ));
        assert!(!is_stale_binary_name(
            "fastctx.exe",
            ".fastctx.exe.fastctx-old-user-backup"
        ));
        assert!(!is_stale_binary_name(
            "fastctx.exe",
            "fastctx.exe~RF1a2B.TMP.user"
        ));
        assert!(!is_stale_binary_name("fastctx.exe", "other.exe~RF1a2B.TMP"));
    }

    #[test]
    fn stale_scan_is_scoped_to_regular_siblings_of_the_exact_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("fastctx.exe");
        let rename_old = temp.path().join(".fastctx.exe.fastctx-old-12.0");
        let replace_file = temp.path().join("fastctx.exe~RF1234.TMP");
        let unrelated = temp.path().join("other.exe~RF1234.TMP");
        std::fs::write(&rename_old, b"old").unwrap();
        std::fs::write(&replace_file, b"old").unwrap();
        std::fs::write(&unrelated, b"user").unwrap();
        std::fs::create_dir(temp.path().join("fastctx.exe~RFdirectory.TMP")).unwrap();

        assert_eq!(
            stale_binary_siblings(&target).unwrap(),
            vec![rename_old, replace_file]
        );
    }
}
