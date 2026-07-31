//! Cross-platform input parsing, output normalization, and filesystem error translation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Converts either user-facing separator style into a path the current platform can parse.
pub fn parse_input_path(input: &str) -> PathBuf {
    if std::path::MAIN_SEPARATOR == '/' {
        PathBuf::from(input.replace('\\', "/"))
    } else {
        PathBuf::from(input)
    }
}

/// Converts a local absolute `file://` URI into the current platform's filesystem path.
///
/// This is a narrow compatibility path for hosts that route local files through MCP resources
/// despite the server omitting the resources capability. Remote authorities and URI decorations
/// are rejected so the fallback cannot silently reinterpret a different resource identifier.
pub(crate) fn local_file_uri_to_path(input: &str) -> Option<PathBuf> {
    if !input
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        return None;
    }
    let url = url::Url::parse(input).ok()?;
    if url.scheme() != "file"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url
            .host_str()
            .is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
    {
        return None;
    }
    let path = url.to_file_path().ok()?;
    path.is_absolute().then_some(path)
}

/// Converts a local absolute path or `file://` URI into a filesystem path.
///
/// Generic MCP resource helpers have emitted both forms for the same local file. Plain paths are
/// accepted only when they are absolute; URI-shaped inputs continue through the stricter
/// `file://` parser above.
pub(crate) fn local_resource_path(input: &str) -> Option<PathBuf> {
    if input
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        return local_file_uri_to_path(input);
    }
    if input.contains("://") {
        return None;
    }
    let path = parse_input_path(input);
    path.is_absolute().then_some(path)
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
    use super::{
        file_url_note, local_file_uri_to_path, local_resource_path, missing_file_message,
        missing_search_path_message,
    };

    #[test]
    fn local_file_uri_conversion_accepts_only_plain_local_absolute_uris() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("resource notes.txt");
        let uri = url::Url::from_file_path(&path).unwrap();
        assert_eq!(local_file_uri_to_path(uri.as_str()), Some(path.clone()));
        assert_eq!(
            local_resource_path(&path.to_string_lossy()),
            Some(path.clone())
        );

        let mut localhost = uri.clone();
        localhost.set_host(Some("localhost")).unwrap();
        assert_eq!(local_file_uri_to_path(localhost.as_str()), Some(path));

        for input in [
            "https://example.com/resource.txt",
            "file:relative.txt",
            "file://server/share/resource.txt",
            "file:///tmp/resource.txt?download=1",
            "file:///tmp/resource.txt#line-1",
            "file://",
        ] {
            assert!(local_file_uri_to_path(input).is_none(), "{input}");
        }
        for input in [
            "relative.txt",
            "https://example.com/resource.txt",
            "file://server/share/resource.txt",
        ] {
            assert!(local_resource_path(input).is_none(), "{input}");
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
}
