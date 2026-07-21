//! Deterministic fixture materialization and readback-derived qualification evidence.

use crate::model::{FixtureEntry, FixtureReadback, FixtureSpec, sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct FixtureGuard {
    root: PathBuf,
    expected: FixtureReadback,
    cleaned: bool,
}

impl FixtureGuard {
    pub fn materialize(root: &Path, spec: &FixtureSpec) -> Result<Self, String> {
        validate_capture_root(root)?;
        if spec.schema != 1 {
            return Err(format!("unsupported fixture schema {}", spec.schema));
        }
        validate_spec(spec)?;
        fs::create_dir(root)
            .map_err(|error| format!("cannot create fixture root {}: {error}", root.display()))?;

        let result = (|| {
            for file in &spec.files {
                let target = root.join(&file.path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "cannot create fixture directory {}: {error}",
                            parent.display()
                        )
                    })?;
                }
                fs::write(&target, file.text.as_bytes()).map_err(|error| {
                    format!("cannot write fixture file {}: {error}", target.display())
                })?;
                set_regular_permissions(&target)?;
                let modified = SystemTime::UNIX_EPOCH
                    .checked_add(Duration::from_secs(file.mtime_unix_seconds))
                    .ok_or_else(|| format!("mtime overflows for {}", file.path))?;
                fs::File::options()
                    .write(true)
                    .open(&target)
                    .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(modified)))
                    .map_err(|error| {
                        format!("cannot set fixture mtime for {}: {error}", target.display())
                    })?;
            }
            readback(root, spec)
        })();

        match result {
            Ok(expected) => Ok(Self {
                root: root.to_path_buf(),
                expected,
                cleaned: false,
            }),
            Err(error) => {
                let _ = fs::remove_dir_all(root);
                Err(error)
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn verify_immutable(&self) -> Result<FixtureReadback, String> {
        let spec = FixtureSpec {
            schema: 1,
            token_budget: 0,
            files: self
                .expected
                .entries
                .iter()
                .filter(|entry| entry.kind == "file")
                .map(|entry| {
                    let path = self.root.join(&entry.path);
                    let text = fs::read_to_string(&path).map_err(|error| {
                        format!("cannot re-read fixture file {}: {error}", path.display())
                    })?;
                    Ok(crate::model::FixtureFile {
                        path: entry.path.clone(),
                        text,
                        mtime_unix_seconds: entry.mtime_unix_seconds,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        };
        let actual = readback(&self.root, &spec)?;
        if actual.fixture_tree_sha256 != self.expected.fixture_tree_sha256
            || actual.entries != self.expected.entries
        {
            return Err(format!(
                "fixture changed during capture: expected {}, got {}",
                self.expected.fixture_tree_sha256, actual.fixture_tree_sha256
            ));
        }
        Ok(actual)
    }

    pub fn finish(mut self) -> Result<(), String> {
        fs::remove_dir_all(&self.root).map_err(|error| {
            format!(
                "cannot remove audited fixture root {}: {error}",
                self.root.display()
            )
        })?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        if !self.cleaned && self.root.exists() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn validate_capture_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(format!(
            "fixture root must be absolute, got {}",
            root.display()
        ));
    }
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "fixture root must have a safe UTF-8 final component".to_string())?;
    if !name.starts_with("fastctx-v011") || !name.is_ascii() {
        return Err(
            "fixture root final component must be ASCII and start with `fastctx-v011`".to_string(),
        );
    }
    if root.parent().is_none() || root.parent() == Some(Path::new("/")) {
        return Err("fixture root must be nested below an explicit parent directory".to_string());
    }
    let parent = root
        .parent()
        .expect("the explicit parent was checked above");
    let git_marker = parent.join(".git");
    let marker_metadata = fs::symlink_metadata(&git_marker).map_err(|error| {
        format!(
            "capture parent must contain an empty .git directory marker at {}: {error}",
            git_marker.display()
        )
    })?;
    if marker_metadata.file_type().is_symlink() || !marker_metadata.is_dir() {
        return Err(format!(
            "capture git marker is not a real directory: {}",
            git_marker.display()
        ));
    }
    if fs::read_dir(&git_marker)
        .map_err(|error| format!("cannot inspect {}: {error}", git_marker.display()))?
        .next()
        .is_some()
    {
        return Err(format!(
            "capture git marker must be empty: {}",
            git_marker.display()
        ));
    }
    if root.exists() {
        return Err(format!(
            "fixture root already exists and will not be overwritten: {}",
            root.display()
        ));
    }
    Ok(())
}

fn validate_spec(spec: &FixtureSpec) -> Result<(), String> {
    if spec.token_budget < 256 {
        return Err("fixture token budget cannot leave the required 256-token slack".to_string());
    }
    let mut seen = BTreeSet::new();
    let mut mtimes = BTreeSet::new();
    for file in &spec.files {
        let path = Path::new(&file.path);
        if path.is_absolute() || file.path.contains('\\') {
            return Err(format!("fixture path is not portable: {}", file.path));
        }
        for component in path.components() {
            let Component::Normal(component) = component else {
                return Err(format!("fixture path escapes its root: {}", file.path));
            };
            let component = component
                .to_str()
                .ok_or_else(|| format!("fixture path is not UTF-8: {}", file.path))?;
            if !safe_component(component) {
                return Err(format!(
                    "fixture path component is forbidden: {component:?}"
                ));
            }
        }
        if !seen.insert(file.path.clone()) {
            return Err(format!("duplicate fixture path: {}", file.path));
        }
        if !mtimes.insert(file.mtime_unix_seconds) {
            return Err(format!(
                "fixture mtimes must be globally unique: {}",
                file.mtime_unix_seconds
            ));
        }
    }
    let mut ordered = mtimes.into_iter();
    if let Some(mut previous) = ordered.next() {
        for current in ordered {
            if current - previous < 10 {
                return Err("fixture target mtimes must differ by at least 10 seconds".to_string());
            }
            previous = current;
        }
    }
    Ok(())
}

fn safe_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.starts_with("~fastctx~b")
        && !component.starts_with("~fastctx~w")
        && !component.chars().any(|character| {
            character == '\\'
                || character == '\u{7f}'
                || character.is_control()
                || matches!(character, '\u{2028}' | '\u{2029}')
        })
}

fn readback(root: &Path, spec: &FixtureSpec) -> Result<FixtureReadback, String> {
    let expected_files = spec
        .files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let mut actual_paths = Vec::new();
    collect_paths(root, root, &mut actual_paths)?;
    actual_paths.sort();

    let mut entries = Vec::new();
    let mut forbidden = Vec::new();
    for relative in actual_paths {
        let full = root.join(&relative);
        let metadata = fs::symlink_metadata(&full)
            .map_err(|error| format!("cannot inspect {}: {error}", full.display()))?;
        if metadata.file_type().is_symlink() {
            forbidden.push(format!("symlink:{relative}"));
            continue;
        }
        if metadata.is_dir() {
            entries.push(FixtureEntry {
                path: relative,
                kind: "directory".to_string(),
                sha256: sha256(&[]),
                bytes: 0,
                mtime_unix_seconds: 0,
                readonly: metadata.permissions().readonly(),
                hard_link_count: 1,
            });
            continue;
        }
        if !metadata.is_file() {
            forbidden.push(format!("special:{relative}"));
            continue;
        }
        if !has_audited_regular_permissions(&metadata) {
            forbidden.push(format!("permissions:{relative}"));
        }
        let Some(expected) = expected_files.get(&relative) else {
            forbidden.push(format!("unexpected-file:{relative}"));
            continue;
        };
        let mut bytes = Vec::new();
        let mut source = fs::File::open(&full)
            .map_err(|error| format!("cannot open {}: {error}", full.display()))?;
        source
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {}: {error}", full.display()))?;
        std::str::from_utf8(&bytes)
            .map_err(|error| format!("fixture file {relative} is not strict UTF-8: {error}"))?;
        if bytes != expected.text.as_bytes() {
            return Err(format!("fixture bytes differ from spec for {relative}"));
        }
        let modified = metadata
            .modified()
            .and_then(|time| {
                time.duration_since(SystemTime::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map_err(|error| format!("cannot read mtime for {relative}: {error}"))?
            .as_secs();
        if modified != expected.mtime_unix_seconds {
            return Err(format!(
                "fixture mtime readback differs for {relative}: expected {}, got {modified}",
                expected.mtime_unix_seconds
            ));
        }
        let links = hard_link_count(&source, &metadata)?;
        if links != 1 {
            forbidden.push(format!("hardlink:{relative}:{links}"));
        }
        entries.push(FixtureEntry {
            path: relative,
            kind: "file".to_string(),
            sha256: sha256(&bytes),
            bytes: bytes.len() as u64,
            mtime_unix_seconds: modified,
            readonly: metadata.permissions().readonly(),
            hard_link_count: links,
        });
    }
    let actual_files = entries
        .iter()
        .filter(|entry| entry.kind == "file")
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let missing = expected_files
        .keys()
        .filter(|path| !actual_files.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("fixture files are missing: {}", missing.join(", ")));
    }
    if !forbidden.is_empty() {
        return Err(format!(
            "fixture contains forbidden features: {}",
            forbidden.join(", ")
        ));
    }
    let tree_bytes = serde_json::to_vec(&entries)
        .map_err(|error| format!("cannot hash fixture readback: {error}"))?;
    Ok(FixtureReadback {
        schema: 1,
        fixture_tree_sha256: sha256(&tree_bytes),
        entries,
        all_components_safe_utf8: true,
        all_contents_strict_utf8: true,
        forbidden_features_found: forbidden,
    })
}

fn collect_paths(root: &Path, directory: &Path, output: &mut Vec<String>) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("cannot read fixture entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("fixture path escaped root: {error}"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| format!("fixture path is not UTF-8: {}", path.display()))?
            .replace('\\', "/");
        output.push(relative);
        if fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .is_dir()
        {
            collect_paths(root, &path, output)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_regular_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
        .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))
}

#[cfg(unix)]
fn has_audited_regular_permissions(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777 == 0o644
}

#[cfg(not(unix))]
fn has_audited_regular_permissions(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)]
fn set_regular_permissions(path: &Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))
}

#[cfg(unix)]
fn hard_link_count(_file: &fs::File, metadata: &fs::Metadata) -> Result<u64, String> {
    use std::os::unix::fs::MetadataExt;
    Ok(metadata.nlink())
}

#[cfg(windows)]
fn hard_link_count(file: &fs::File, _metadata: &fs::Metadata) -> Result<u64, String> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `file` owns a valid live handle and `information` points to writable storage for
    // exactly the structure required by GetFileInformationByHandle.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(format!(
            "cannot read the fixture hard-link count: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: a nonzero return value guarantees that Windows initialized the output structure.
    let information = unsafe { information.assume_init() };
    Ok(u64::from(information.nNumberOfLinks))
}

#[cfg(not(any(unix, windows)))]
fn hard_link_count(_file: &fs::File, _metadata: &fs::Metadata) -> Result<u64, String> {
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::FixtureGuard;
    use crate::model::{FixtureFile, FixtureSpec};

    #[test]
    fn fixture_readback_detects_content_changes() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join(".git")).unwrap();
        let root = parent.path().join("fastctx-v011-fixture");
        let fixture = FixtureGuard::materialize(
            &root,
            &FixtureSpec {
                schema: 1,
                token_budget: 8_500,
                files: vec![
                    FixtureFile {
                        path: "a.txt".to_string(),
                        text: "alpha\n".to_string(),
                        mtime_unix_seconds: 2_000_000_000,
                    },
                    FixtureFile {
                        path: "nested/b.txt".to_string(),
                        text: "beta\n".to_string(),
                        mtime_unix_seconds: 2_000_000_020,
                    },
                ],
            },
        )
        .unwrap();
        fixture.verify_immutable().unwrap();
        std::fs::write(root.join("a.txt"), b"changed\n").unwrap();
        assert!(fixture.verify_immutable().is_err());
    }

    #[test]
    fn fixture_root_refuses_existing_or_unscoped_targets() {
        let spec = FixtureSpec {
            schema: 1,
            token_budget: 8_500,
            files: vec![FixtureFile {
                path: "a.txt".to_string(),
                text: "alpha\n".to_string(),
                mtime_unix_seconds: 2_000_000_000,
            }],
        };
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join(".git")).unwrap();
        assert!(FixtureGuard::materialize(&parent.path().join("fixture"), &spec).is_err());
        assert!(
            FixtureGuard::materialize(&parent.path().join("prefix-fastctx-v011-fixture"), &spec)
                .is_err()
        );
        let existing = parent.path().join("fastctx-v011-existing");
        std::fs::create_dir(&existing).unwrap();
        assert!(FixtureGuard::materialize(&existing, &spec).is_err());
    }
}
