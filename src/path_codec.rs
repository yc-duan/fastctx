//! Lossless search-path identity, display encoding, and root resolution.

use crate::paths::canonical_existing;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

const BYTE_TOKEN_PREFIX: &str = "~fastctx~b";
const WINDOWS_TOKEN_PREFIX: &str = "~fastctx~w";
const TOKEN_SUFFIX: char = '~';
const INVALID_TOKEN_SUFFIX: &str =
    "for this platform. Copy an encoded path exactly as returned by FastCtx.";

/// The native, lossless final tie-break key for a path.
#[cfg(unix)]
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NativePathKey(Vec<u8>);

/// The native, lossless final tie-break key for a path.
#[cfg(windows)]
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NativePathKey(Vec<u16>);

/// Metadata observed by traversal before a candidate is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentityHint {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    #[cfg(unix)]
    pub(crate) device: u64,
    #[cfg(unix)]
    pub(crate) inode: u64,
    #[cfg(unix)]
    pub(crate) modified_seconds: i64,
    #[cfg(unix)]
    pub(crate) modified_nanoseconds: i64,
    #[cfg(unix)]
    pub(crate) changed_seconds: i64,
    #[cfg(unix)]
    pub(crate) changed_nanoseconds: i64,
    #[cfg(windows)]
    pub(crate) creation_time: u64,
    #[cfg(windows)]
    pub(crate) last_write_time: u64,
    #[cfg(windows)]
    pub(crate) attributes: u32,
}

impl FileIdentityHint {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: metadata.len(),
                modified: metadata.modified().ok(),
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Self {
                len: metadata.len(),
                modified: metadata.modified().ok(),
                creation_time: metadata.creation_time(),
                last_write_time: metadata.last_write_time(),
                attributes: metadata.file_attributes(),
            }
        }
    }
}

/// One candidate path with separate native identity and canonical model-facing keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PathRecord {
    pub(crate) native: PathBuf,
    pub(crate) display: Arc<str>,
    pub(crate) relative_match: Arc<str>,
    pub(crate) native_key: NativePathKey,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) traversal_len_hint: Option<u64>,
    pub(crate) traversal_fingerprint: Option<FileIdentityHint>,
}

impl PathRecord {
    /// Captures lossless path keys without performing metadata IO.
    pub(crate) fn without_metadata(native: &Path, match_root: &Path) -> Self {
        Self {
            native: native.to_path_buf(),
            display: Arc::from(display_path(native)),
            relative_match: Arc::from(display_path(
                native.strip_prefix(match_root).unwrap_or(native),
            )),
            native_key: native_path_key(native),
            modified: None,
            traversal_len_hint: None,
            traversal_fingerprint: None,
        }
    }

    /// Captures walker metadata, optionally requiring the modification-time sort key.
    pub(crate) fn from_metadata(
        native: &Path,
        match_root: &Path,
        metadata: &Metadata,
        include_modified: bool,
    ) -> io::Result<Self> {
        let mut record = Self::without_metadata(native, match_root);
        record.modified = if include_modified {
            Some(metadata.modified()?)
        } else {
            None
        };
        record.traversal_len_hint = Some(metadata.len());
        record.traversal_fingerprint = Some(FileIdentityHint::from_metadata(metadata));
        Ok(record)
    }
}

/// Whether a resolved grep root is a single file or a directory tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootKind {
    File,
    Directory,
}

/// A search root whose one metadata result is reused by every downstream decision.
#[derive(Debug)]
pub(crate) struct ResolvedRoot {
    pub(crate) native: PathBuf,
    pub(crate) display: Arc<str>,
    pub(crate) metadata: Metadata,
    pub(crate) kind: RootKind,
}

impl ResolvedRoot {
    /// Wraps metadata already obtained by a legacy caller without another stat.
    pub(crate) fn from_metadata(native: PathBuf, metadata: Metadata) -> Result<Self, String> {
        let kind = metadata_kind(&native, &metadata)?;
        Ok(Self {
            display: Arc::from(display_path(&native)),
            native,
            metadata,
            kind,
        })
    }

    pub(crate) fn is_file(&self) -> bool {
        self.kind == RootKind::File
    }

    pub(crate) fn match_root(&self) -> &Path {
        if self.is_file() {
            self.native.parent().unwrap_or(&self.native)
        } else {
            &self.native
        }
    }
}

/// The accepted filesystem shape for a tool's search root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootRequirement {
    FileOrDirectory,
    Directory,
}

/// An invalid complete FastCtx filename escape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PathCodecError {
    component: String,
}

impl PathCodecError {
    fn new(component: &str) -> Self {
        Self {
            component: component.to_string(),
        }
    }
}

impl fmt::Display for PathCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Invalid FastCtx-encoded path component \"{}\" {INVALID_TOKEN_SUFFIX}",
            self.component
        )
    }
}

impl std::error::Error for PathCodecError {}

/// Parses a model-facing path and decodes each canonical normal component exactly once.
pub(crate) fn parse_input_path(input: &str) -> Result<PathBuf, PathCodecError> {
    let mut decoded = PathBuf::new();
    for component in Path::new(input).components() {
        match component {
            Component::Prefix(prefix) => decoded.push(prefix.as_os_str()),
            Component::RootDir => decoded.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => decoded.push("."),
            Component::ParentDir => decoded.push(".."),
            Component::Normal(component) => decoded.push(decode_component_once(component)?),
        }
    }
    Ok(decoded)
}

/// Produces a slash-separated path whose normal components are injective and line-safe.
pub(crate) fn display_path(path: &Path) -> String {
    let mut display = String::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => append_prefix(&mut display, prefix),
            Component::RootDir => {
                if !display.ends_with('/') {
                    display.push('/');
                }
            }
            Component::CurDir => append_display_component(&mut display, "."),
            Component::ParentDir => append_display_component(&mut display, ".."),
            Component::Normal(component) => {
                let encoded = encode_component(component);
                append_display_component(&mut display, &encoded);
            }
        }
    }
    display
}

/// Translates search-path IO failures without lossy or line-breaking path rendering.
pub(crate) fn io_error_message(path: &Path, error: &io::Error) -> String {
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

/// Resolves a grep/glob root with one explicit metadata call and no existence preflight.
pub(crate) fn resolve_search_root(
    input: Option<&str>,
    requirement: RootRequirement,
) -> Result<ResolvedRoot, String> {
    let parsed = match input {
        Some(input) => parse_input_path(input).map_err(|error| error.to_string())?,
        None => std::env::current_dir()
            .map_err(|error| format!("Cannot access the session working directory: {error}"))?,
    };

    if input.is_some() && !parsed.is_absolute() {
        return Err(missing_relative_search_path_message(&parsed));
    }

    let metadata = root_metadata(&parsed).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            missing_absolute_search_path_message(&parsed)
        } else {
            io_error_message(&parsed, &error)
        }
    })?;
    let kind = metadata_kind(&parsed, &metadata)?;
    if requirement == RootRequirement::Directory && kind != RootKind::Directory {
        return Err(format!(
            "Path is not a directory: {}",
            display_path(&parsed)
        ));
    }

    let canonical =
        canonical_existing(&parsed).map_err(|error| io_error_message(&parsed, &error))?;
    Ok(ResolvedRoot {
        display: Arc::from(display_path(&canonical)),
        native: canonical,
        metadata,
        kind,
    })
}

fn metadata_kind(path: &Path, metadata: &Metadata) -> Result<RootKind, String> {
    if metadata.is_file() {
        Ok(RootKind::File)
    } else if metadata.is_dir() {
        Ok(RootKind::Directory)
    } else {
        Err(format!(
            "Path is not a file or directory: {}",
            display_path(path)
        ))
    }
}

fn missing_absolute_search_path_message(parsed: &Path) -> String {
    format!(
        "Path does not exist: {}\nNote: the session working directory is {}.",
        display_path(parsed),
        current_dir_display()
    )
}

fn missing_relative_search_path_message(parsed: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved = cwd.join(parsed);
    let mut note = format!(
        "Note: the session working directory is {}.",
        current_dir_display()
    );
    if fs::metadata(&resolved).is_ok()
        && let Ok(absolute) = canonical_existing(&resolved)
    {
        note.push_str(&format!(
            " Use the absolute path {}.",
            display_path(&absolute)
        ));
    }
    format!("Path does not exist: {}\n{note}", display_path(parsed))
}

fn current_dir_display() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| canonical_existing(&path).ok().or(Some(path)))
        .map(|path| display_path(&path))
        .unwrap_or_else(|| ".".to_string())
}

#[cfg(test)]
thread_local! {
    static ROOT_METADATA_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn root_metadata(path: &Path) -> io::Result<Metadata> {
    #[cfg(test)]
    ROOT_METADATA_CALLS.with(|calls| calls.set(calls.get() + 1));
    fs::metadata(path)
}

fn encode_component(component: &OsStr) -> String {
    if let Some(text) = component.to_str() {
        if component_is_safe(text) {
            return text.to_string();
        }
        return byte_token(text.as_bytes());
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        byte_token(component.as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        windows_token(component.encode_wide())
    }
}

fn component_is_safe(component: &str) -> bool {
    !component
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
        && (!cfg!(unix) || !component.contains('\\'))
        && canonical_token(component).is_none()
}

fn decode_component_once(component: &OsStr) -> Result<OsString, PathCodecError> {
    let Some(text) = component.to_str() else {
        return Ok(component.to_os_string());
    };
    let Some(token) = canonical_token(text) else {
        return Ok(component.to_os_string());
    };

    let decoded = match token {
        CanonicalToken::Bytes(payload) => decode_byte_component(payload, text)?,
        CanonicalToken::Windows(payload) => decode_windows_component(payload, text)?,
    };
    if !is_single_normal_component(&decoded) {
        return Err(PathCodecError::new(text));
    }
    Ok(decoded)
}

fn decode_byte_component(payload: &str, source: &str) -> Result<OsString, PathCodecError> {
    let bytes = decode_byte_pairs(payload);
    #[cfg(unix)]
    let decoded = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(bytes.clone())
    };
    #[cfg(windows)]
    let decoded = String::from_utf8(bytes.clone())
        .map(OsString::from)
        .map_err(|_| PathCodecError::new(source))?;

    if byte_token(&bytes) != source {
        return Err(PathCodecError::new(source));
    }
    Ok(decoded)
}

#[cfg(unix)]
fn decode_windows_component(_payload: &str, source: &str) -> Result<OsString, PathCodecError> {
    Err(PathCodecError::new(source))
}

#[cfg(windows)]
fn decode_windows_component(payload: &str, source: &str) -> Result<OsString, PathCodecError> {
    use std::os::windows::ffi::OsStringExt;
    let units = payload
        .as_bytes()
        .chunks_exact(4)
        .map(decode_hex_u16)
        .collect::<Vec<_>>();
    if windows_token(units.iter().copied()) != source {
        return Err(PathCodecError::new(source));
    }
    Ok(OsString::from_wide(&units))
}

fn is_single_normal_component(component: &OsStr) -> bool {
    if native_component_has_forbidden_separator(component) {
        return false;
    }
    let mut components = Path::new(component).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(unix)]
fn native_component_has_forbidden_separator(component: &OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    component
        .as_bytes()
        .iter()
        .any(|byte| matches!(*byte, 0 | b'/'))
}

#[cfg(windows)]
fn native_component_has_forbidden_separator(component: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    component
        .encode_wide()
        .any(|unit| unit == 0 || unit == u16::from(b'/') || unit == u16::from(b'\\'))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CanonicalToken<'a> {
    Bytes(&'a str),
    Windows(&'a str),
}

fn canonical_token(component: &str) -> Option<CanonicalToken<'_>> {
    if let Some(payload) = component
        .strip_prefix(BYTE_TOKEN_PREFIX)
        .and_then(|payload| payload.strip_suffix(TOKEN_SUFFIX))
        && !payload.is_empty()
        && payload.len().is_multiple_of(2)
        && payload.bytes().all(is_lower_hex)
    {
        return Some(CanonicalToken::Bytes(payload));
    }

    if let Some(payload) = component
        .strip_prefix(WINDOWS_TOKEN_PREFIX)
        .and_then(|payload| payload.strip_suffix(TOKEN_SUFFIX))
        && !payload.is_empty()
        && payload.len().is_multiple_of(4)
        && payload.bytes().all(is_lower_hex)
    {
        return Some(CanonicalToken::Windows(payload));
    }
    None
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn byte_token(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(BYTE_TOKEN_PREFIX.len() + bytes.len() * 2 + 1);
    encoded.push_str(BYTE_TOKEN_PREFIX);
    append_lower_hex(&mut encoded, bytes);
    encoded.push(TOKEN_SUFFIX);
    encoded
}

#[cfg(windows)]
fn windows_token(units: impl IntoIterator<Item = u16>) -> String {
    use std::fmt::Write;
    let mut encoded = String::from(WINDOWS_TOKEN_PREFIX);
    for unit in units {
        write!(&mut encoded, "{unit:04x}").expect("writing to a String cannot fail");
    }
    encoded.push(TOKEN_SUFFIX);
    encoded
}

fn append_lower_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn decode_byte_pairs(payload: &str) -> Vec<u8> {
    payload
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| (decode_hex_nibble(pair[0]) << 4) | decode_hex_nibble(pair[1]))
        .collect()
}

fn decode_hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("canonical token grammar already validated lowercase hex"),
    }
}

#[cfg(windows)]
fn decode_hex_u16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0_u16, |value, byte| {
        (value << 4) | u16::from(decode_hex_nibble(*byte))
    })
}

#[cfg(unix)]
fn native_path_key(path: &Path) -> NativePathKey {
    use std::os::unix::ffi::OsStrExt;
    NativePathKey(path.as_os_str().as_bytes().to_vec())
}

#[cfg(windows)]
fn native_path_key(path: &Path) -> NativePathKey {
    use std::os::windows::ffi::OsStrExt;
    NativePathKey(path.as_os_str().encode_wide().collect())
}

fn append_display_component(display: &mut String, component: &str) {
    if !display.is_empty() && !display.ends_with('/') {
        display.push('/');
    }
    display.push_str(component);
}

#[cfg(unix)]
fn append_prefix(_display: &mut str, prefix: std::path::PrefixComponent<'_>) {
    unreachable!("prefix components cannot be produced by Unix paths: {prefix:?}")
}

#[cfg(windows)]
fn append_prefix(display: &mut String, prefix: std::path::PrefixComponent<'_>) {
    use std::path::Prefix;
    match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
            display.push((drive as char).to_ascii_uppercase());
            display.push(':');
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            display.push_str("//");
            display.push_str(&encode_component(server));
            display.push('/');
            display.push_str(&encode_component(share));
        }
        Prefix::DeviceNS(device) => {
            display.push_str("//./");
            display.push_str(&encode_component(device));
        }
        Prefix::Verbatim(value) => display.push_str(&encode_component(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ROOT_METADATA_CALLS, RootKind, RootRequirement, display_path, io_error_message,
        parse_input_path, resolve_search_root,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn portable_special_unicode_has_hard_coded_canonical_bytes() {
        assert_eq!(
            display_path(Path::new("line\u{2028}break.txt")),
            "~fastctx~b6c696e65e280a8627265616b2e747874~"
        );
        assert_eq!(
            display_path(Path::new("line\u{2029}break.txt")),
            "~fastctx~b6c696e65e280a9627265616b2e747874~"
        );
        assert_eq!(
            display_path(Path::new("tab\tname.txt")),
            "~fastctx~b746162096e616d652e747874~"
        );
    }

    #[test]
    fn ordinary_safe_unicode_components_remain_verbatim() {
        for path in [
            "alpha.txt",
            "nested/child.rs",
            "目录/文件.txt",
            "emoji-😀.txt",
            "~fastctx~notes",
            "~fastctx~bzz~",
        ] {
            assert_eq!(display_path(Path::new(path)), path);
            assert_eq!(parse_input_path(path).unwrap(), PathBuf::from(path));
        }
    }

    #[test]
    fn every_unicode_control_component_is_line_safe_and_reversible() {
        for character in (1_u32..=31).chain([127]) {
            let character = char::from_u32(character).unwrap();
            let native = format!("left{character}right");
            let encoded = display_path(Path::new(&native));
            assert!(encoded.starts_with("~fastctx~b"), "{encoded}");
            assert!(!encoded.chars().any(char::is_control), "{encoded:?}");
            assert_eq!(parse_input_path(&encoded).unwrap(), PathBuf::from(native));
        }
    }

    #[test]
    fn canonical_literal_is_encoded_and_outer_token_decodes_only_once() {
        let literal = "~fastctx~b61~";
        let outer = "~fastctx~b7e666173746374787e6236317e~";
        assert_eq!(display_path(Path::new(literal)), outer);
        assert_eq!(parse_input_path(outer).unwrap(), PathBuf::from(literal));
        assert_eq!(
            parse_input_path("~fastctx~b61~").unwrap(),
            PathBuf::from("a")
        );
    }

    #[test]
    fn malformed_token_shapes_are_literals_but_valid_dangerous_payloads_fail_closed() {
        for literal in [
            "~fastctx~notes",
            "~fastctx~backup",
            "~fastctx~bzz~",
            "~fastctx~bA0~",
            "~fastctx~b0~",
            "~fastctx~x61~",
        ] {
            assert_eq!(parse_input_path(literal).unwrap(), PathBuf::from(literal));
            assert_eq!(display_path(Path::new(literal)), literal);
        }
        for invalid in ["~fastctx~b00~", "~fastctx~b2f~", "~fastctx~b2e~"] {
            assert_eq!(
                parse_input_path(invalid).unwrap_err().to_string(),
                format!(
                    "Invalid FastCtx-encoded path component \"{invalid}\" for this platform. Copy an encoded path exactly as returned by FastCtx."
                )
            );
        }
    }

    #[test]
    fn successful_root_resolution_uses_one_explicit_metadata_call() {
        let temp = tempfile::tempdir().unwrap();
        ROOT_METADATA_CALLS.with(|calls| calls.set(0));
        let root = resolve_search_root(
            Some(temp.path().to_str().unwrap()),
            RootRequirement::Directory,
        )
        .unwrap();
        assert_eq!(root.native, dunce::canonicalize(temp.path()).unwrap());
        ROOT_METADATA_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    }

    #[test]
    fn file_and_missing_root_resolution_each_use_one_explicit_metadata_call() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("single.txt");
        std::fs::write(&file, b"stable").unwrap();

        ROOT_METADATA_CALLS.with(|calls| calls.set(0));
        let resolved = resolve_search_root(
            Some(file.to_str().unwrap()),
            RootRequirement::FileOrDirectory,
        )
        .unwrap();
        assert_eq!(resolved.kind, RootKind::File);
        ROOT_METADATA_CALLS.with(|calls| assert_eq!(calls.get(), 1));

        let missing = temp.path().join("missing.txt");
        ROOT_METADATA_CALLS.with(|calls| calls.set(0));
        let error = resolve_search_root(
            Some(missing.to_str().unwrap()),
            RootRequirement::FileOrDirectory,
        )
        .unwrap_err();
        assert!(error.starts_with(&format!(
            "Path does not exist: {}\n",
            display_path(&missing)
        )));
        ROOT_METADATA_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    }

    #[test]
    fn io_errors_keep_specific_kind_and_line_safe_path() {
        let path = Path::new("line\nname.txt");
        let display = "~fastctx~b6c696e650a6e616d652e747874~";
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert_eq!(
            io_error_message(path, &denied),
            format!("Permission denied: {display}")
        );

        let other = std::io::Error::other("disk fault");
        assert_eq!(
            io_error_message(path, &other),
            format!("Cannot open file: {display} (disk fault)")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_sharing_violation_has_the_locked_error_contract() {
        let error = std::io::Error::from_raw_os_error(32);
        assert_eq!(
            io_error_message(Path::new(r"C:\locked.txt"), &error),
            "Cannot open file (locked by another process): C:/locked.txt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_invalid_bytes_and_literal_backslash_are_lossless_and_distinct() {
        use std::ffi::OsString;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let invalid = PathBuf::from(OsString::from_vec(vec![0xff]));
        assert_eq!(display_path(&invalid), "~fastctx~bff~");
        assert_eq!(
            parse_input_path("~fastctx~bff~")
                .unwrap()
                .as_os_str()
                .as_bytes(),
            &[0xff]
        );
        assert_eq!(
            display_path(Path::new("a\\b.txt")),
            "~fastctx~b615c622e747874~"
        );
        assert_ne!(
            display_path(Path::new("a\\b.txt")),
            display_path(Path::new("a/b.txt"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_single_byte_components_are_injective_and_round_trip() {
        use std::collections::BTreeSet;
        use std::ffi::OsString;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let mut displays = BTreeSet::new();
        for byte in 1_u8..=u8::MAX {
            if matches!(byte, b'/' | b'.') {
                continue;
            }
            let native = PathBuf::from(OsString::from_vec(vec![byte]));
            let display = display_path(&native);
            assert!(displays.insert(display.clone()), "collision at {byte:#x}");
            assert_eq!(
                parse_input_path(&display).unwrap().as_os_str().as_bytes(),
                &[byte]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn canonical_windows_token_is_reserved_and_rejected_on_unix() {
        let token = "~fastctx~w0061~";
        assert_eq!(
            parse_input_path(token).unwrap_err().to_string(),
            "Invalid FastCtx-encoded path component \"~fastctx~w0061~\" for this platform. Copy an encoded path exactly as returned by FastCtx."
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogate_uses_w_token_and_round_trips() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let native = PathBuf::from(OsString::from_wide(&[0xd800]));
        assert_eq!(display_path(&native), "~fastctx~wd800~");
        assert_eq!(
            parse_input_path("~fastctx~wd800~")
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            vec![0xd800]
        );
        assert!(parse_input_path("~fastctx~bff~").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogate_sequences_are_injective() {
        use std::collections::BTreeSet;
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let cases: &[&[u16]] = &[
            &[0xd800],
            &[0xdfff],
            &[0x0061, 0xd800],
            &[0xd800, 0x0061],
            &[0xd800, 0xdfff, 0xd800],
        ];
        let mut displays = BTreeSet::new();
        for units in cases {
            let native = PathBuf::from(OsString::from_wide(units));
            let display = display_path(&native);
            assert!(displays.insert(display.clone()), "{display}");
            assert_eq!(
                parse_input_path(&display)
                    .unwrap()
                    .as_os_str()
                    .encode_wide()
                    .collect::<Vec<_>>(),
                *units
            );
        }
    }
}
