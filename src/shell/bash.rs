//! Deterministic bash discovery with explicit-override semantics.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const WINDOWS_MISSING_BASH: &str = "Cannot find a usable bash. fastshell runs every command with bash for consistent cross-platform behavior. Install Git for Windows (https://git-scm.com/downloads) or set FASTCTX_BASH to the absolute path of bash.exe. Note: C:/Windows/System32/bash.exe is the WSL launcher, not a standalone bash.";
const UNIX_MISSING_BASH: &str =
    "Cannot find a usable bash. Install bash or set FASTCTX_BASH to its absolute path.";

/// Caches the validated backend for one fastshell server process.
#[derive(Debug, Default)]
pub(crate) struct BashLocator {
    cached: OnceLock<Result<PathBuf, String>>,
}

impl BashLocator {
    /// Returns the validated bash path, probing at most once per server process.
    pub(crate) fn resolve(&self) -> Result<PathBuf, String> {
        self.cached.get_or_init(probe_bash).clone()
    }
}

/// Probes bash without caching, used by Apply preflight and doctor.
pub(crate) fn probe_bash() -> Result<PathBuf, String> {
    match std::env::var("FASTCTX_BASH") {
        Ok(value) => return validate_override(&value),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("Invalid FASTCTX_BASH value \"<non-UTF-8>\": not a working bash (the path is not valid UTF-8). Fix or unset it.".to_string());
        }
        Err(std::env::VarError::NotPresent) => {}
    }

    let mut seen = HashSet::new();
    for candidate in automatic_candidates() {
        if excluded_candidate(&candidate) || !seen.insert(candidate_key(&candidate)) {
            continue;
        }
        if validate_bash(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(missing_bash_message().to_string())
}

fn validate_override(value: &str) -> Result<PathBuf, String> {
    let path = crate::paths::parse_input_path(value);
    let result = if !path.is_absolute() {
        Err("the path is not absolute".to_string())
    } else if excluded_candidate(&path) {
        Err("the path points to the Windows/WSL launcher or a WindowsApps shim".to_string())
    } else {
        validate_bash(&path)
    };
    result.map(|_| path).map_err(|reason| {
        format!(
            "Invalid FASTCTX_BASH value \"{value}\": not a working bash ({reason}). Fix or unset it."
        )
    })
}

fn validate_bash(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the path is not absolute".to_string());
    }
    if !path.is_file() {
        return Err("the file does not exist".to_string());
    }
    let output = crate::process_policy::noninteractive_command(path)
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!(
            "--version exited {}",
            output.status.code().unwrap_or(1)
        ));
    }
    if !text.contains("GNU bash") {
        return Err("--version output did not contain GNU bash".to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn automatic_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for git in path_candidates("git.exe") {
        let mut ancestor = git.parent();
        for _ in 0..4 {
            let Some(directory) = ancestor else { break };
            candidates.push(directory.join("usr").join("bin").join("bash.exe"));
            ancestor = directory.parent();
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(root).join("Git/usr/bin/bash.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LocalAppData") {
        candidates.push(PathBuf::from(root).join("Programs/Git/usr/bin/bash.exe"));
    }
    candidates.extend(path_candidates("bash.exe"));
    candidates
}

#[cfg(not(windows))]
fn automatic_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("/bin/bash"), PathBuf::from("/usr/bin/bash")];
    candidates.extend(path_candidates("bash"));
    candidates
}

fn path_candidates(name: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(windows)]
fn excluded_candidate(path: &Path) -> bool {
    let canonical = crate::paths::canonical_existing(path).unwrap_or_else(|_| path.to_path_buf());
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| crate::paths::canonical_existing(&root).unwrap_or(root));
    excluded_windows_candidate(&canonical, root.as_deref())
}

#[cfg(windows)]
fn excluded_windows_candidate(path: &Path, system_root: Option<&Path>) -> bool {
    let candidate = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    if candidate.contains("\\windowsapps\\") {
        return true;
    }
    system_root
        .map(|root| {
            let root = root
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_ascii_lowercase();
            candidate == root || candidate.starts_with(&format!("{root}\\"))
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn excluded_candidate(_path: &Path) -> bool {
    false
}

fn candidate_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn missing_bash_message() -> &'static str {
    if cfg!(windows) {
        WINDOWS_MISSING_BASH
    } else {
        UNIX_MISSING_BASH
    }
}

#[cfg(test)]
mod tests {
    use super::{candidate_key, validate_override};

    #[test]
    fn explicit_override_rejects_relative_paths_without_fallback() {
        let error = validate_override("relative/bash").unwrap_err();
        assert_eq!(
            error,
            "Invalid FASTCTX_BASH value \"relative/bash\": not a working bash (the path is not absolute). Fix or unset it."
        );
    }

    #[test]
    fn candidate_keys_follow_platform_path_case_rules() {
        let key = candidate_key(std::path::Path::new("C:\\Tools\\Bash.exe"));
        if cfg!(windows) {
            assert_eq!(key, "c:/tools/bash.exe");
        } else {
            assert_eq!(key, "C:/Tools/Bash.exe");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_exclusion_rejects_systemroot_and_windowsapps_without_path_probing() {
        use super::excluded_windows_candidate;
        let root = std::path::Path::new("C:/Windows");
        assert!(excluded_windows_candidate(
            std::path::Path::new("C:/Windows/System32/bash.exe"),
            Some(root)
        ));
        assert!(excluded_windows_candidate(
            std::path::Path::new("C:/Users/test/AppData/Local/Microsoft/WindowsApps/bash.exe"),
            Some(root)
        ));
        assert!(!excluded_windows_candidate(
            std::path::Path::new("C:/Program Files/Git/usr/bin/bash.exe"),
            Some(root)
        ));
    }
}
