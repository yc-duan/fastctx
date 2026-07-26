//! Cross-platform input parsing, output normalization, and filesystem error translation.

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Immutable canonical read-root authorization used by the MCP file tools.
///
/// `None` preserves FastCtx's historical unrestricted behavior when the server
/// was started without `--allow-root`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReadScope {
    roots: Option<Arc<[AllowedRoot]>>,
}

#[derive(Clone, Debug)]
struct AllowedRoot {
    canonical: PathBuf,
    capability: Arc<Dir>,
}

/// A path routed to an immutable startup capability. The relative locator is
/// never interpreted by ambient filesystem APIs.
#[derive(Clone)]
pub(crate) struct ScopedPath {
    pub(crate) canonical: PathBuf,
    pub(crate) relative: PathBuf,
    pub(crate) capability: Arc<Dir>,
}

impl ReadScope {
    pub(crate) fn unrestricted() -> Self {
        Self { roots: None }
    }

    pub(crate) fn is_restricted(&self) -> bool {
        self.roots.is_some()
    }

    /// Canonicalizes and validates every configured root once at startup.
    pub(crate) fn from_allow_roots(roots: &[PathBuf]) -> Result<Self, String> {
        if roots.is_empty() {
            return Ok(Self::unrestricted());
        }
        let mut canonical = Vec::with_capacity(roots.len());
        for root in roots {
            if !root.is_absolute() {
                return Err(format!(
                    "Invalid --allow-root {}: the path must be absolute.",
                    display_path(root)
                ));
            }
            let metadata = fs::metadata(root).map_err(|error| {
                format!(
                    "Invalid --allow-root {}: cannot access the directory ({error}).",
                    display_path(root)
                )
            })?;
            if !metadata.is_dir() {
                return Err(format!(
                    "Invalid --allow-root {}: the path is not a directory.",
                    display_path(root)
                ));
            }
            let root = canonical_existing(root).map_err(|error| {
                format!(
                    "Invalid --allow-root {}: cannot canonicalize the directory ({error}).",
                    display_path(root)
                )
            })?;
            let capability =
                Dir::open_ambient_dir(&root, ambient_authority()).map_err(|error| {
                    format!(
                        "Invalid --allow-root {}: cannot open the directory capability ({error}).",
                        display_path(&root)
                    )
                })?;
            canonical.push(AllowedRoot {
                canonical: root,
                capability: Arc::new(capability),
            });
        }
        canonical.sort_by(|left, right| left.canonical.cmp(&right.canonical));
        canonical.dedup_by(|left, right| left.canonical == right.canonical);
        Ok(Self {
            roots: Some(Arc::from(canonical)),
        })
    }

    /// Canonicalizes a target (including stable symlink targets) before callers
    /// perform any metadata or content read, and enforces component-aware
    /// containment against the configured roots.
    pub(crate) fn authorize(&self, path: &Path) -> Result<PathBuf, String> {
        self.authorize_with_formatter(path, display_path)
    }

    pub(crate) fn authorize_with_formatter(
        &self,
        path: &Path,
        formatter: fn(&Path) -> String,
    ) -> Result<PathBuf, String> {
        let Some(roots) = &self.roots else {
            return Ok(canonical_existing(path).unwrap_or_else(|_| path.to_path_buf()));
        };
        let canonical = canonical_for_authorization(path).map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                self.denied_with_formatter(path, formatter)
            } else {
                io_error_message_with_formatter(path, &error, formatter)
            }
        })?;
        if roots
            .iter()
            .any(|root| canonical.starts_with(&root.canonical))
        {
            Ok(canonical)
        } else {
            Err(self.denied_with_formatter(path, formatter))
        }
    }

    fn denied_with_formatter(&self, path: &Path, formatter: fn(&Path) -> String) -> String {
        format!("Permission denied: {}", formatter(path))
    }

    /// Routes a request to the longest matching startup capability and returns
    /// a capability-relative locator. Canonicalization here is only routing;
    /// all subsequent metadata, open, and traversal operations use `capability`.
    pub(crate) fn route_with_formatter(
        &self,
        path: &Path,
        formatter: fn(&Path) -> String,
    ) -> Result<ScopedPath, String> {
        let canonical = canonical_for_authorization(path).map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                self.denied_with_formatter(path, formatter)
            } else {
                io_error_message_with_formatter(path, &error, formatter)
            }
        })?;
        let Some(roots) = &self.roots else {
            let mut root = canonical.parent().unwrap_or(canonical.as_path());
            loop {
                match Dir::open_ambient_dir(root, ambient_authority()) {
                    Ok(capability) => {
                        let relative = canonical
                            .strip_prefix(root)
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|_| PathBuf::from("."));
                        return Ok(ScopedPath {
                            canonical,
                            relative: if relative.as_os_str().is_empty() {
                                PathBuf::from(".")
                            } else {
                                relative
                            },
                            capability: Arc::new(capability),
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        let Some(parent) = root.parent() else {
                            return Err(io_error_message_with_formatter(path, &error, formatter));
                        };
                        root = parent;
                    }
                    Err(error) => {
                        return Err(io_error_message_with_formatter(path, &error, formatter));
                    }
                }
            }
        };
        let root = roots
            .iter()
            .filter(|root| canonical.starts_with(&root.canonical))
            .max_by_key(|root| root.canonical.components().count())
            .ok_or_else(|| self.denied_with_formatter(path, formatter))?;
        let relative = canonical
            .strip_prefix(&root.canonical)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::from("."));
        Ok(ScopedPath {
            canonical,
            relative: if relative.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                relative
            },
            capability: Arc::clone(&root.capability),
        })
    }

    pub(crate) fn route(&self, path: &Path) -> Result<ScopedPath, String> {
        self.route_with_formatter(path, display_path)
    }
}

fn canonical_for_authorization(path: &Path) -> io::Result<PathBuf> {
    match canonical_existing(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let name = path.file_name().ok_or(error)?;
            let parent = path.parent().unwrap_or_else(|| Path::new("/"));
            Ok(canonical_for_authorization(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

fn io_error_message_with_formatter(
    path: &Path,
    error: &io::Error,
    formatter: fn(&Path) -> String,
) -> String {
    let display = formatter(path);
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return format!("Cannot open file (locked by another process): {display}");
    }
    if error.kind() == io::ErrorKind::PermissionDenied {
        return format!("Permission denied: {display}");
    }
    format!("Cannot open file: {display} ({error})")
}

/// Converts either user-facing separator style into a path the current platform can parse.
pub fn parse_input_path(input: &str) -> PathBuf {
    if std::path::MAIN_SEPARATOR == '/' {
        PathBuf::from(input.replace('\\', "/"))
    } else {
        PathBuf::from(input)
    }
}

pub(crate) fn absolute_path_required_message(input: &str) -> String {
    format!("Path must be absolute: {}", input.replace('\\', "/"))
}

/// Returns an absolute display path that never depends on platform backslashes.
pub fn display_path(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = value.strip_prefix("//?/UNC/") {
        value = format!("//{rest}");
    } else if let Some(rest) = value.strip_prefix("//?/") {
        value = rest.to_string();
    }
    value
}

/// Canonicalizes an existing path to a stable absolute form without the Windows `\\?\` prefix.
pub fn canonical_existing(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Normalized display value of the current session working directory.
pub fn current_dir_display() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| canonical_existing(&path).ok().or(Some(path)))
        .map(|path| display_path(&path))
        .unwrap_or_else(|| ".".to_string())
}

/// Translates open/read failures into the frozen permission or lock messages.
pub fn io_error_message(path: &Path, error: &io::Error) -> String {
    let display = display_path(path);
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return format!("Cannot open file (locked by another process): {display}");
    }
    if error.kind() == io::ErrorKind::PermissionDenied {
        return format!("Permission denied: {display}");
    }
    format!("Cannot open file: {display} ({error})")
}

/// Selects a high-confidence sibling candidate for a missing file.
pub fn nearest_existing_name(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let wanted = path.file_name()?.to_string_lossy();
    let mut candidates = fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let score = strsim::jaro_winkler(&wanted, &name);
            (score, name, entry.path())
        })
        .filter(|(_, name, _)| name != wanted.as_ref())
        .filter(|(score, _, _)| *score >= 0.80)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    candidates.into_iter().next().map(|(_, _, path)| path)
}

/// Recovery note for an input written as a `file://` URL instead of a filesystem path.
///
/// Hosts hand the model file URLs of their own (resource reads, editor selections), so a caller
/// that has just been redirected to these tools often keeps the URL form; naming the plain path is
/// what turns a second failure into a working call. Returns `None` for anything else.
fn file_url_note(input: &str) -> Option<String> {
    let rest = input
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("file://"))
        .map(|_| &input[7..])?;
    // Strip an empty or "localhost" authority, then the slash that precedes a Windows drive letter.
    let rest = rest.strip_prefix("localhost/").unwrap_or(rest);
    let decoded = percent_decoded(rest);
    let path = decoded
        .strip_prefix('/')
        .filter(|tail| {
            let mut characters = tail.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.next() == Some(':')
        })
        .unwrap_or(&decoded);
    (!path.is_empty())
        .then(|| format!("\nNote: this is a file:// URL, not a path. Use {path} instead."))
}

/// Decodes `%XX` escapes, leaving malformed escapes as written.
fn percent_decoded(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| value.get(index + 1..index + 3))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        match escape {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

/// Builds the read error for missing or relative paths, including a recovery step when possible.
pub fn missing_file_message(input: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parsed = parse_input_path(input);
    let resolved = if parsed.is_absolute() {
        parsed.clone()
    } else {
        cwd.join(&parsed)
    };
    let requested = if parsed.is_absolute() {
        display_path(&parsed)
    } else {
        input.replace('\\', "/")
    };
    let cwd_display = current_dir_display();
    let mut note = format!("Note: the session working directory is {cwd_display}.");
    if !parsed.is_absolute() && resolved.exists() {
        let resolved_display = canonical_existing(&resolved).unwrap_or_else(|_| resolved.clone());
        note.push_str(&format!(
            " Use the absolute path {}.",
            display_path(&resolved_display)
        ));
    }
    let mut message = format!("File does not exist: {requested}\n{note}");
    if let Some(url_note) = file_url_note(input) {
        message.push_str(&url_note);
        return message;
    }
    if let Some(candidate) = nearest_existing_name(&resolved) {
        let candidate = canonical_existing(&candidate).unwrap_or(candidate);
        message.push_str(&format!("\nDid you mean: {}?", display_path(&candidate)));
    }
    message
}

/// Builds read's missing-file error and explains why a lossy U+FFFD filename cannot round-trip as text.
pub fn missing_read_file_message(input: &str) -> String {
    let mut message = missing_file_message(input);
    if input.contains('\u{FFFD}') {
        message.push_str(
            "\nNote: this path contains U+FFFD (a placeholder for bytes that are not valid text); it looks like the lossy rendering of a filename that cannot be represented as text and cannot be opened by name.",
        );
    }
    message
}

/// Builds the missing-path error shared by grep and glob.
pub fn missing_search_path_message(input: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let parsed = parse_input_path(input);
    let resolved = if parsed.is_absolute() {
        parsed
    } else {
        cwd.join(&parsed)
    };
    let mut note = format!(
        "Note: the session working directory is {}.",
        current_dir_display()
    );
    if !Path::new(input).is_absolute() && resolved.exists() {
        let absolute = canonical_existing(&resolved).unwrap_or(resolved);
        note.push_str(&format!(
            " Use the absolute path {}.",
            display_path(&absolute)
        ));
    }
    let mut message = format!("Path does not exist: {}\n{note}", input.replace('\\', "/"));
    if let Some(url_note) = file_url_note(input) {
        message.push_str(&url_note);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::{ReadScope, file_url_note, missing_file_message, missing_search_path_message};
    use std::path::{Path, PathBuf};

    fn assert_denied(scope: &ReadScope, path: &Path) {
        for result in [
            scope.authorize(path).map(|_| ()),
            scope.route(path).map(|_| ()),
        ] {
            let message = result.unwrap_err();
            assert!(message.starts_with("Permission denied: "), "{message}");
        }
    }

    #[test]
    fn file_urls_are_translated_to_the_plain_path() {
        for (input, expected) in [
            ("file:///V:/repo/AGENTS.md", "V:/repo/AGENTS.md"),
            ("FILE:///V:/repo/AGENTS.md", "V:/repo/AGENTS.md"),
            ("file://localhost/V:/repo/AGENTS.md", "V:/repo/AGENTS.md"),
            ("file:///home/user/notes.md", "/home/user/notes.md"),
            ("file:///V:/repo/my%20notes.md", "V:/repo/my notes.md"),
            ("file:///V:/repo/%E4%B8%AD%E6%96%87.md", "V:/repo/中文.md"),
        ] {
            let note = file_url_note(input).unwrap_or_else(|| panic!("no note for {input}"));
            assert_eq!(
                note,
                format!("\nNote: this is a file:// URL, not a path. Use {expected} instead."),
                "{input}"
            );
        }
    }

    #[test]
    fn plain_paths_and_other_schemes_get_no_url_note() {
        // A leading slash that is not a drive letter must survive, and non-file schemes are
        // somebody else's problem — guessing at them would invent a path that does not exist.
        for input in [
            "V:/repo/AGENTS.md",
            "/home/user/notes.md",
            "https://example.com/a.md",
            "notafile://x",
            "file://",
        ] {
            assert!(file_url_note(input).is_none(), "{input}");
        }
    }

    #[test]
    fn missing_file_and_search_errors_carry_the_recovery_path() {
        let read = missing_file_message("file:///V:/definitely/missing.md");
        assert!(
            read.ends_with("Use V:/definitely/missing.md instead."),
            "{read}"
        );
        let search = missing_search_path_message("file:///V:/definitely/missing");
        assert!(
            search.ends_with("Use V:/definitely/missing instead."),
            "{search}"
        );
    }

    #[test]
    fn allow_root_validation_rejects_relative_missing_and_non_directory_paths() {
        let relative = ReadScope::from_allow_roots(&[PathBuf::from("relative")]).unwrap_err();
        assert!(relative.contains("must be absolute"), "{relative}");

        let missing = tempfile::tempdir().unwrap().path().join("missing");
        let missing_error = ReadScope::from_allow_roots(&[missing]).unwrap_err();
        assert!(
            missing_error.contains("cannot access the directory"),
            "{missing_error}"
        );

        let fixture = tempfile::tempdir().unwrap();
        let file = fixture.path().join("file.txt");
        std::fs::write(&file, b"content").unwrap();
        let non_directory = ReadScope::from_allow_roots(&[file]).unwrap_err();
        assert!(non_directory.contains("not a directory"), "{non_directory}");
    }

    #[cfg(unix)]
    #[test]
    fn capability_roots_route_symlink_hubs_only_when_target_root_is_configured() {
        let fixture = tempfile::tempdir().unwrap();
        let root_a = fixture.path().join("a");
        let root_b = fixture.path().join("b");
        std::fs::create_dir(&root_a).unwrap();
        std::fs::create_dir(&root_b).unwrap();
        std::fs::write(root_b.join("inside.txt"), b"cross-root").unwrap();
        let hub = root_a.join("hub");
        std::os::unix::fs::symlink(&root_b, &hub).unwrap();
        let target = hub.join("inside.txt");
        let only_a = ReadScope::from_allow_roots(std::slice::from_ref(&root_a)).unwrap();
        assert!(only_a.route(&target).is_err());
        let both = ReadScope::from_allow_roots(&[root_a, root_b]).unwrap();
        let routed = both.route(&target).unwrap();
        assert_eq!(routed.canonical, dunce::canonicalize(&target).unwrap());
    }

    #[test]
    fn allow_root_authorization_is_component_aware() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("work");
        let sibling = fixture.path().join("work-secret");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        let inside = root.join("inside.txt");
        let outside = sibling.join("outside.txt");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let routed = scope.route(&inside).unwrap();
        assert_eq!(routed.canonical, dunce::canonicalize(&inside).unwrap());
        assert_eq!(routed.relative, PathBuf::from("inside.txt"));
        assert_denied(&scope, &outside);
        assert_denied(
            &scope,
            &root.join("..").join("work-secret").join("outside.txt"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn allow_root_routing_denies_static_file_symlinks_to_outside_targets() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&outside, b"secret-target-content").unwrap();
        let link = root.join("outside-link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        assert_denied(&scope, &link);
    }
}
