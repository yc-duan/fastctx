//! Byte-preserving editable text snapshots, line anchors, CAS, and atomic commit.

use crate::control::transaction;
use crate::encoding::{EditableDecodedText, EncodingDecision, ValidatedFileEncoding};
use crate::paths::{display_path, io_error_message, missing_file_message, parse_input_path};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
type BeforeCommitHook = Box<dyn FnOnce(&Path)>;

#[cfg(test)]
thread_local! {
    static BEFORE_COMMIT: std::cell::RefCell<Option<BeforeCommitHook>> =
        std::cell::RefCell::new(None);
}

pub(crate) const MAX_EDIT_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_REPLACE_RESULT_BYTES: usize = 256 * 1024 * 1024;

/// Newline style used for every boundary introduced by an edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EolStyle {
    Lf,
    Crlf,
}

impl EolStyle {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

struct LogicalView {
    logical: String,
    raw_boundaries: Vec<Option<usize>>,
    eol: EolStyle,
}

/// Frozen source bytes and every derived view needed for safe line edits.
#[derive(Clone, Debug)]
pub(crate) struct TextDocument {
    requested_path: PathBuf,
    target_path: PathBuf,
    raw: Vec<u8>,
    validated: ValidatedFileEncoding,
    logical: String,
    logical_raw_boundaries: Vec<Option<usize>>,
    eol: EolStyle,
    trailing_newline: bool,
    unix_mode: Option<u32>,
}

impl TextDocument {
    /// Opens one absolute regular-file target, following symlinks while preserving the link itself.
    pub(crate) fn open(file_path: &str, encoding: Option<&str>) -> Result<Self, String> {
        let requested_path = parse_input_path(file_path);
        if !requested_path.is_absolute() {
            return Err(missing_file_message(file_path));
        }
        match fs::symlink_metadata(&requested_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(missing_file_message(file_path));
            }
            Err(error) => return Err(io_error_message(&requested_path, &error)),
        }
        let target_path = resolve_target(&requested_path)?;
        reject_hard_link(&target_path)?;
        let metadata =
            fs::metadata(&target_path).map_err(|error| io_error_message(&target_path, &error))?;
        if !metadata.is_file() {
            return Err(format!(
                "Cannot edit non-regular file: {}. Only regular files are supported.",
                display_path(&requested_path)
            ));
        }
        if metadata.len() > MAX_EDIT_FILE_BYTES {
            return Err(format!(
                "File too large for line edits: {} is {:.1} MiB (limit: 64 MiB).",
                display_path(&requested_path),
                metadata.len() as f64 / 1_048_576.0
            ));
        }
        let raw = fs::read(&target_path).map_err(|error| io_error_message(&target_path, &error))?;
        let validated = match crate::encoding::validate_file_encoding(&target_path, encoding)
            .map_err(|error| io_error_message(&target_path, &error))?
        {
            EncodingDecision::Text(validated) => validated,
            EncodingDecision::Binary => {
                return Err(format!(
                    "Cannot read binary file as text: {}. Use view=\"hex\" to inspect its raw bytes.",
                    display_path(&requested_path)
                ));
            }
            EncodingDecision::Rejected(rejection) => {
                return Err(rejection.message(&display_path(&requested_path)));
            }
        };
        let current =
            fs::read(&target_path).map_err(|error| io_error_message(&target_path, &error))?;
        if current != raw {
            return Err(concurrent_message(&requested_path));
        }
        let editable = validated.decode_editable_snapshot(&raw).map_err(|reason| {
            format!(
                "Cannot safely edit {}: {reason}. Convert the file to UTF-8 externally and retry.",
                display_path(&requested_path)
            )
        })?;
        let LogicalView {
            logical,
            raw_boundaries: logical_raw_boundaries,
            eol,
        } = logical_view(&editable)?;
        let trailing_newline = logical.ends_with('\n');
        let unix_mode = transaction::existing_unix_mode(&target_path);
        Ok(Self {
            requested_path,
            target_path,
            raw,
            validated,
            logical,
            logical_raw_boundaries,
            eol,
            trailing_newline,
            unix_mode,
        })
    }

    pub(crate) fn display_path(&self) -> String {
        display_path(&self.requested_path)
    }

    pub(crate) fn target_path(&self) -> &Path {
        &self.target_path
    }

    pub(crate) fn original_bytes(&self) -> &[u8] {
        &self.raw
    }

    pub(crate) fn logical_text(&self) -> &str {
        &self.logical
    }

    pub(crate) fn trailing_newline(&self) -> bool {
        self.trailing_newline
    }

    pub(crate) fn encoding_label(&self) -> &'static str {
        self.validated.encoding_label()
    }

    pub(crate) fn revision(&self) -> String {
        hex::encode(Sha256::digest(&self.raw))
    }

    /// Commits one frozen snapshot if and only if the target still equals B0.
    pub(crate) fn commit(&self, new_bytes: &[u8]) -> Result<(), String> {
        #[cfg(test)]
        run_before_commit_hook(&self.target_path);
        let current = fs::read(&self.target_path)
            .map_err(|error| io_error_message(&self.target_path, &error))?;
        if current != self.raw {
            return Err(concurrent_message(&self.requested_path));
        }
        reject_hard_link(&self.target_path)?;
        transaction::atomic_replace(&self.target_path, new_bytes, self.unix_mode, false)
    }

    pub(crate) fn encode_for_target(&self, logical_text: &str) -> Result<Vec<u8>, String> {
        let text = logical_text.replace('\n', self.eol.as_str());
        self.validated.encode_fragment(&text).ok_or_else(|| {
            format!(
                "Cannot write {}: the replacement text cannot be encoded as {}. Convert the file to UTF-8 externally or use replacement text representable in that encoding.",
                self.display_path(),
                self.encoding_label()
            )
        })
    }

    pub(crate) fn raw_offset(&self, logical_offset: usize) -> Result<usize, String> {
        self.logical_raw_boundaries
            .get(logical_offset)
            .and_then(|offset| *offset)
            .ok_or_else(|| {
                "Internal edit failure: a logical range did not end on a character boundary."
                    .to_string()
            })
    }
}

#[cfg(test)]
pub(crate) fn set_before_commit_hook(hook: impl FnOnce(&Path) + 'static) {
    BEFORE_COMMIT.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_before_commit_hook(path: &Path) {
    BEFORE_COMMIT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook(path);
        }
    });
}

fn logical_view(editable: &EditableDecodedText) -> Result<LogicalView, String> {
    let mut logical = String::with_capacity(editable.text.len());
    let mut boundaries = vec![None; editable.text.len().saturating_add(1)];
    boundaries[0] = editable.raw_boundaries[0];
    let mut crlf = 0_usize;
    let mut lf = 0_usize;
    let mut cursor = 0_usize;
    while cursor < editable.text.len() {
        let character = editable.text[cursor..]
            .chars()
            .next()
            .expect("cursor remains on a character boundary");
        let next = cursor + character.len_utf8();
        if character == '\r' && editable.text[next..].starts_with('\n') {
            let decoded_end = next + 1;
            let logical_start = logical.len();
            logical.push('\n');
            boundaries.resize(logical.len() + 1, None);
            boundaries[logical_start] = editable.raw_boundaries[cursor];
            boundaries[logical.len()] = editable.raw_boundaries[decoded_end];
            crlf += 1;
            cursor = decoded_end;
            continue;
        }
        let logical_start = logical.len();
        logical.push(character);
        boundaries.resize(logical.len() + 1, None);
        boundaries[logical_start] = editable.raw_boundaries[cursor];
        boundaries[logical.len()] = editable.raw_boundaries[next];
        if character == '\n' {
            lf += 1;
        }
        cursor = next;
    }
    let eol = if crlf > lf {
        EolStyle::Crlf
    } else {
        EolStyle::Lf
    };
    Ok(LogicalView {
        logical,
        raw_boundaries: boundaries,
        eol,
    })
}

fn resolve_target(requested: &Path) -> Result<PathBuf, String> {
    let metadata =
        fs::symlink_metadata(requested).map_err(|error| io_error_message(requested, &error))?;
    if metadata.file_type().is_symlink() {
        let target = crate::paths::canonical_existing(requested).map_err(|_| {
            format!(
                "Cannot edit {}: it is a symbolic link that does not resolve to a file.",
                display_path(requested)
            )
        })?;
        if !target.is_file() {
            return Err(format!(
                "Cannot edit {}: it is a symbolic link that does not resolve to a file.",
                display_path(requested)
            ));
        }
        Ok(target)
    } else {
        Ok(crate::paths::canonical_existing(requested).unwrap_or_else(|_| requested.to_path_buf()))
    }
}

fn reject_hard_link(path: &Path) -> Result<(), String> {
    if hard_link_count(path)? > 1 {
        return Err(format!(
            "Cannot safely edit {}: it has multiple hard links, and an atomic replace would break them. Duplicate the file to a new path, or remove the extra links first.",
            display_path(path)
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn hard_link_count(path: &Path) -> Result<u64, String> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .map(|metadata| metadata.nlink())
        .map_err(|error| io_error_message(path, &error))
}

#[cfg(windows)]
fn hard_link_count(path: &Path) -> Result<u64, String> {
    use std::fs::File;
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(path).map_err(|error| io_error_message(path, &error))?;
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let success =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if success == 0 {
        return Err(io_error_message(path, &std::io::Error::last_os_error()));
    }
    Ok(unsafe { info.assume_init() }.nNumberOfLinks as u64)
}

#[cfg(not(any(unix, windows)))]
fn hard_link_count(_path: &Path) -> Result<u64, String> {
    Ok(1)
}

fn concurrent_message(path: &Path) -> String {
    format!(
        "{} changed on disk during the edit; nothing was written. Re-read it and retry.",
        display_path(path)
    )
}

#[cfg(test)]
mod tests {
    use super::TextDocument;

    #[test]
    fn utf16_mapping_preserves_every_unmodified_raw_byte() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("utf16.txt");
        let mut raw = vec![0xff, 0xfe];
        for unit in "one\r\ntwo".encode_utf16() {
            raw.extend(unit.to_le_bytes());
        }
        std::fs::write(&path, &raw).unwrap();
        let document = TextDocument::open(path.to_str().unwrap(), None).unwrap();
        let start = document.logical_text().find("two").unwrap();
        let end = start + "two".len();
        let raw_start = document.raw_offset(start).unwrap();
        let raw_end = document.raw_offset(end).unwrap();
        let mut result = Vec::new();
        result.extend_from_slice(&document.original_bytes()[..raw_start]);
        result.extend_from_slice(&document.encode_for_target("TWO").unwrap());
        result.extend_from_slice(&document.original_bytes()[raw_end..]);
        assert_eq!(&result[..12], &raw[..12]);
        assert_eq!(&result[..2], &[0xff, 0xfe]);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_uses_the_specific_recovery_error() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("dangling.txt");
        symlink(temp.path().join("missing.txt"), &link).unwrap();
        let error = TextDocument::open(link.to_str().unwrap(), None).unwrap_err();
        assert_eq!(
            error,
            format!(
                "Cannot edit {}: it is a symbolic link that does not resolve to a file.",
                crate::paths::display_path(&link)
            )
        );
    }
}
