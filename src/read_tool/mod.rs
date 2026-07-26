//! Text, image, PDF, and raw-byte dispatch for the read tool.

mod batch;
mod hex_file;
mod image_file;
#[cfg(feature = "pdf")]
mod pdf;
#[cfg(not(feature = "pdf"))]
#[path = "pdf_disabled.rs"]
mod pdf;
#[cfg(feature = "pdf")]
mod pdf_engine;
mod text_file;

use crate::binary::detect_binary_type;
use crate::budget::{READ_TOKEN_BUDGET_ENV, tool_token_budget};
use crate::model::ToolResponse;
use crate::paths::{
    ReadScope, absolute_path_required_message, display_path, io_error_message,
    missing_read_file_message, parse_input_path,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

/// Line window for a text read that omits `limit`: unbounded, so the token budget is the only
/// ceiling on how much one call returns.
///
/// A second fixed window would silently cap every default read far below the configured budget,
/// forcing continuation calls the budget was raised to avoid. Do not reintroduce a numeric default
/// here; page explicitly with `limit` instead. Locked by
/// `read_contract::default_text_read_is_bounded_only_by_the_token_budget`. (2026-07-25)
const UNBOUNDED_LINE_LIMIT: usize = usize::MAX;
/// Line window for a hex read that omits `limit`.
///
/// Hex keeps a fixed default where text does not: a dump exists to inspect a byte range rather
/// than to deliver a whole binary, and its candidate window is estimated from the budget with a
/// deliberately loose factor, so an unbounded default would buffer hundreds of thousands of
/// rendered lines before the budget trimmed them back. (2026-07-25)
const DEFAULT_HEX_LINE_LIMIT: usize = 2_000;
const MAX_LINE_CHARS: usize = 2_000;
const TOTAL_COUNT_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

/// Automatic read dispatch or raw-byte viewing.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ViewMode {
    /// Select the text, image, or PDF channel from the file type.
    #[default]
    Auto,
    /// Return a paged hexadecimal dump of the raw bytes.
    Hex,
}

/// Parameters for the read tool; offset is a one-based line number.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    /// The absolute path to the file to read. Both / and \ are accepted. Mutually exclusive with files.
    pub file_path: Option<String>,
    /// Batch form: an array of {"path", "offset"?, "limit"?, "encoding"?} objects for
    /// reading 1-32 text files in one call. Each entry behaves like a single-file text read;
    /// results are packed in request order. Mutually exclusive with file_path and with the
    /// top-level offset/limit/encoding/pages/pdf_mode/view parameters.
    #[schemars(length(min = 1, max = 32))]
    pub files: Option<Vec<BatchReadEntry>>,
    /// The 1-based line number to start reading from. Use for paging through large files.
    #[schemars(range(min = 1))]
    pub offset: Option<usize>,
    /// The number of lines to read. Omit to read as much as the output budget holds.
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    /// Page range for PDF files, e.g. "1-5", "3", "10-20". Max 20 pages per call. Required in text mode for PDFs with more than 10 pages.
    pub pages: Option<String>,
    /// PDF only: "text" (default) returns the selected pages' text layer; "image" returns each selected page rendered as a PNG image.
    #[schemars(with = "Option<pdf::PdfMode>")]
    pub pdf_mode: Option<String>,
    /// Text files only. Known source encoding as a WHATWG label, e.g. "gbk", "shift_jis", "big5", "euc-kr", "windows-1252", "utf-16le", plus "utf-32le"/"utf-32be". Selects how source bytes are decoded; output is always UTF-8. Omit for auto-detection; set it when you know the source encoding or the tool reports an ambiguous or undecodable encoding.
    pub encoding: Option<String>,
    /// "auto" (default) picks the channel by file type; "hex" returns a paged hex dump of the raw bytes of any file — the way to inspect binary files.
    #[schemars(with = "Option<ViewMode>")]
    pub view: Option<String>,
}

/// One text file in a batch read request.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchReadEntry {
    /// Absolute path of the text file. Both / and \ are accepted.
    pub path: String,
    /// The 1-based line number to start reading from.
    #[schemars(range(min = 1))]
    pub offset: Option<usize>,
    /// Maximum lines to read from this file in this call. Omit to let the shared budget decide.
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    /// Known source encoding for this file, using the same labels as single-file read.
    pub encoding: Option<String>,
}

/// Reads text, images, PDFs, or raw bytes and surfaces every expected failure explicitly.
pub fn read_file(request: ReadRequest) -> ToolResponse {
    read_file_with_scope(request, &ReadScope::unrestricted())
}

pub(crate) fn read_file_with_scope(request: ReadRequest, scope: &ReadScope) -> ToolResponse {
    match (request.file_path.as_deref(), request.files.as_ref()) {
        (Some(_), Some(_)) | (None, None) => {
            return ToolResponse::error("Provide exactly one of file_path or files.");
        }
        (None, Some(_)) => return batch::read_text_files(request, scope),
        (Some(_), None) => {}
    }
    let file_path = request
        .file_path
        .as_deref()
        .expect("single-file shape was validated");
    let parsed = parse_input_path(file_path);
    if !parsed.is_absolute() {
        if scope.is_restricted() {
            return ToolResponse::error(absolute_path_required_message(file_path));
        }
        return ToolResponse::error(missing_read_file_message(file_path));
    }
    let view = if scope.is_restricted() {
        match validate_view_parameters(&request) {
            Ok(view) => Some(view),
            Err(message) => return ToolResponse::error(message),
        }
    } else {
        None
    };
    let routed = match scope.route(&parsed) {
        Ok(routed) => routed,
        Err(message) => return ToolResponse::error(message),
    };
    let metadata = match routed.capability.metadata(&routed.relative) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ToolResponse::error(missing_read_file_message(file_path));
        }
        Err(error) => return ToolResponse::error(io_error_message(&parsed, &error)),
    };
    if metadata.is_dir() {
        return ToolResponse::error(format!(
            "{} is a directory, not a file. Use the glob tool to list its contents.",
            display_path(&routed.canonical)
        ));
    }
    if !metadata.is_file() {
        return ToolResponse::error(format!(
            "Cannot read non-regular file: {}. Only regular files are supported.",
            display_path(&routed.canonical)
        ));
    }
    let mut file = match routed.capability.open(&routed.relative) {
        Ok(file) => file.into_std(),
        Err(error) => return ToolResponse::error(io_error_message(&parsed, &error)),
    };
    #[cfg(test)]
    crate::file_snapshot::tests::notify_original_open(&routed.canonical);
    let prefix = match read_prefix(&mut file) {
        Ok(prefix) => prefix,
        Err(error) => {
            return ToolResponse::error(io_error_message(&parsed, &error));
        }
    };
    let view = match view {
        Some(view) => view,
        None => match validate_view_parameters(&request) {
            Ok(view) => view,
            Err(message) => return ToolResponse::error(message),
        },
    };
    if view == ViewMode::Hex {
        let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
            Ok(budget) => budget,
            Err(message) => return ToolResponse::error(message),
        };
        return hex_file::read_hex_handle(
            file,
            &routed.canonical,
            request.offset,
            request.limit,
            budget,
        );
    }
    if pdf::is_pdf(&routed.canonical, &prefix) {
        if request.encoding.is_some() {
            return ToolResponse::error("The encoding parameter only applies to text files.");
        }
        let mode = match pdf::parse_pdf_mode(request.pdf_mode.as_deref()) {
            Ok(mode) => mode,
            Err(message) => return ToolResponse::error(message),
        };
        let budget = if mode == pdf::PdfMode::Text {
            match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
                Ok(budget) => Some(budget),
                Err(message) => return ToolResponse::error(message),
            }
        } else {
            None
        };
        return pdf::read_pdf_handle(
            file,
            &routed.canonical,
            request.pages.as_deref(),
            mode,
            budget,
        );
    }
    if request.pages.is_some() {
        return ToolResponse::error("The pages parameter only applies to PDF files.");
    }
    if request.pdf_mode.is_some() {
        return ToolResponse::error("The pdf_mode parameter only applies to PDF files.");
    }
    if image_file::detect_image_mime(&routed.canonical, &prefix).is_some() {
        if request.encoding.is_some() {
            return ToolResponse::error("The encoding parameter only applies to text files.");
        }
        return image_file::read_image_handle(file, &routed.canonical);
    }
    let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => return ToolResponse::error(message),
    };
    text_file::read_text_handle(
        &file,
        &routed.canonical,
        &display_path(&routed.canonical),
        request.offset,
        request.limit,
        request.encoding.as_deref(),
        detect_binary_type(&prefix),
        budget,
    )
}

pub(super) fn read_prefix(file: &mut fs::File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = Vec::new();
    file.take(8 * 1024).read_to_end(&mut prefix)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(prefix)
}

fn parse_view(value: Option<&str>) -> Result<ViewMode, String> {
    match value {
        None | Some("auto") => Ok(ViewMode::Auto),
        Some("hex") => Ok(ViewMode::Hex),
        Some(value) => Err(format!(
            "Invalid view value \"{value}\". Use \"auto\" or \"hex\"."
        )),
    }
}

fn validate_view_parameters(request: &ReadRequest) -> Result<ViewMode, String> {
    let view = parse_view(request.view.as_deref())?;
    if view == ViewMode::Hex {
        for (parameter, present) in [
            ("pdf_mode", request.pdf_mode.is_some()),
            ("pages", request.pages.is_some()),
            ("encoding", request.encoding.is_some()),
        ] {
            if present {
                return Err(format!(
                    "The {parameter} parameter cannot be combined with view=\"hex\"."
                ));
            }
        }
    }
    Ok(view)
}

fn binary_error(path_display: &str, binary_type: Option<&str>) -> ToolResponse {
    ToolResponse::error(binary_error_message(path_display, binary_type))
}

fn binary_error_message(path_display: &str, binary_type: Option<&str>) -> String {
    let kind = binary_type.map_or_else(String::new, |kind| format!(" (looks like {kind})"));
    format!(
        "Cannot read binary file as text: {path_display}{kind}. Use view=\"hex\" to inspect its raw bytes."
    )
}

#[cfg(test)]
mod tests {
    use super::{BatchReadEntry, ReadRequest, read_file, read_file_with_scope};
    use crate::ToolContent;
    use crate::paths::{ReadScope, display_path, missing_read_file_message};
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn single(path: &std::path::Path) -> ReadRequest {
        single_input(display_path(path))
    }

    fn single_input(path: String) -> ReadRequest {
        ReadRequest {
            file_path: Some(path),
            files: None,
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        }
    }

    fn batch_entry(path: String) -> BatchReadEntry {
        BatchReadEntry {
            path,
            offset: None,
            limit: None,
            encoding: None,
        }
    }

    fn text_entry(path: &std::path::Path) -> BatchReadEntry {
        batch_entry(display_path(path))
    }

    fn batch(entries: impl IntoIterator<Item = BatchReadEntry>) -> ReadRequest {
        ReadRequest {
            file_path: None,
            files: Some(entries.into_iter().collect()),
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        }
    }

    fn scope_for(root: &std::path::Path, restricted: bool) -> ReadScope {
        if restricted {
            ReadScope::from_allow_roots(&[root.to_path_buf()]).unwrap()
        } else {
            ReadScope::unrestricted()
        }
    }

    #[cfg(unix)]
    fn replace_file_after_open(
        path: &std::path::Path,
        old_path: &std::path::Path,
        replacement: &'static [u8],
    ) -> crate::file_snapshot::tests::OriginalOpenObserverGuard {
        let path = path.to_path_buf();
        let old_path = old_path.to_path_buf();
        crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(move |opened| {
            if opened == path && path.exists() {
                fs::rename(&path, &old_path).unwrap();
                fs::write(&path, replacement).unwrap();
            }
        }))
    }

    fn response_text(response: crate::ToolResponse) -> String {
        let [ToolContent::Text(text)] = response.content.as_slice() else {
            panic!("expected one text response: {response:?}");
        };
        text.clone()
    }

    #[test]
    fn unrestricted_missing_parent_keeps_missing_diagnostics_before_view_validation() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("missing-parent").join("input.txt");
        let path_display = display_path(&path);
        let expected = missing_read_file_message(&path_display);

        let mut request = single(&path);
        request.view = Some("invalid".to_string());
        assert_eq!(response_text(read_file(request)), expected);

        let batch = batch([batch_entry(path_display)]);
        assert!(
            response_text(read_file(batch)).contains(&expected),
            "batch missing-parent diagnostics changed"
        );
    }

    #[test]
    fn restricted_invalid_single_parameters_do_not_open_the_file() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let path = root.join("input.txt");
        fs::write(&path, b"content").unwrap();
        let opened = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&opened);
        let _hook =
            crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(move |_| {
                observed.store(true, Ordering::Release)
            }));
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let mut request = single(&path);
        request.view = Some("invalid".to_string());
        let response = read_file_with_scope(request, &scope);
        assert!(response.is_error);
        assert!(!opened.load(Ordering::Acquire));

        let mut request = single(&path);
        request.view = Some("hex".to_string());
        request.encoding = Some("utf-8".to_string());
        let response = read_file_with_scope(request, &scope);
        assert!(response.is_error);
        assert!(!opened.load(Ordering::Acquire));
    }

    #[test]
    fn restricted_small_hex_and_text_pages_do_not_create_sealed_snapshots() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let hex_path = root.join("large.bin");
        let text_path = root.join("large.txt");
        fs::write(&hex_path, vec![b'x'; 9 * 1024 * 1024]).unwrap();
        fs::write(&text_path, b"line\n".repeat(9 * 1024 * 1024 / 5)).unwrap();
        let snapshots = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&snapshots);
        let _observer =
            crate::file_snapshot::tests::TempCreateObserverGuard::install(Arc::new(move |_| {
                observed.fetch_add(1, Ordering::AcqRel);
            }));
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        let mut hex = single(&hex_path);
        hex.view = Some("hex".to_string());
        hex.limit = Some(1);
        let hex_response = read_file_with_scope(hex, &scope);
        assert!(!hex_response.is_error, "{hex_response:?}");

        let mut text = single(&text_path);
        text.limit = Some(1);
        let text_response = read_file_with_scope(text, &scope);
        assert!(!text_response.is_error, "{text_response:?}");
        assert!(response_text(text_response).contains("1\tline"));
        assert_eq!(snapshots.load(Ordering::Acquire), 0);
    }

    #[test]
    fn restricted_text_matches_unrestricted_full_encoding_contract() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let mut invalid_tail = b"valid-prefix\n".repeat(800);
        invalid_tail.push(0xFF);
        let mut legacy_tail = b"ascii-prefix\n".repeat(800);
        legacy_tail.extend_from_slice(b"\xC4\xE3\n");
        let mut iso_2022_tail = b"valid-prefix\n".repeat(800);
        iso_2022_tail.extend_from_slice(b"\x1B$");
        let cases = [
            ("invalid-tail.txt", invalid_tail, None, None),
            ("legacy-tail.txt", legacy_tail, Some("gbk"), None),
            ("iso-2022-tail.txt", iso_2022_tail, None, None),
            ("empty.txt", Vec::new(), None, None),
            ("unbounded-default.txt", b"\n".repeat(2_099), None, None),
            (
                "partial.txt",
                b"first\nsecond\nthird\n".to_vec(),
                None,
                Some(1),
            ),
        ];
        for (name, bytes, encoding, limit) in cases {
            let path = root.join(name);
            fs::write(&path, bytes).unwrap();
            let mut request = single(&path);
            request.encoding = encoding.map(str::to_string);
            request.limit = limit;
            let unrestricted = response_text(read_file(request.clone()));
            let restricted = response_text(read_file_with_scope(request, &scope));
            assert_eq!(restricted, unrestricted, "{name}");
        }
    }

    #[test]
    fn restricted_batch_matches_unrestricted_text_contract() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        fs::write(&first, b"first\nsecond\n").unwrap();
        fs::write(&second, b"third\nfourth\n").unwrap();
        let mut first_entry = text_entry(&first);
        first_entry.offset = Some(2);
        first_entry.limit = Some(1);
        let request = batch([first_entry, text_entry(&second)]);
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        assert_eq!(
            response_text(read_file_with_scope(request.clone(), &scope)),
            response_text(read_file(request))
        );
    }

    #[cfg(unix)]
    #[test]
    fn restricted_alias_display_matches_unrestricted_single_batch_and_diagnostics() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let target = root.join("target.txt");
        let alias = root.join("alias.txt");
        let directory_alias = root.join("directory-alias");
        fs::write(&target, b"alias content\n").unwrap();
        std::os::unix::fs::symlink("target.txt", &alias).unwrap();
        std::os::unix::fs::symlink(".", &directory_alias).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        assert_eq!(
            response_text(read_file_with_scope(single(&alias), &scope)),
            response_text(read_file(single(&alias)))
        );
        let batch = batch([text_entry(&alias)]);
        assert_eq!(
            response_text(read_file_with_scope(batch.clone(), &scope)),
            response_text(read_file(batch))
        );
        assert_eq!(
            response_text(read_file_with_scope(single(&directory_alias), &scope)),
            response_text(read_file(single(&directory_alias)))
        );
    }

    #[test]
    fn restricted_image_rejects_text_encoding_like_unrestricted_reads() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let path = root.join("image.png");
        fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let mut request = single(&path);
        request.encoding = Some("utf-8".to_string());
        assert_eq!(
            response_text(read_file_with_scope(request, &scope)),
            "The encoding parameter only applies to text files."
        );
    }

    #[cfg(unix)]
    #[test]
    fn restricted_image_and_pdf_keep_using_the_open_capability_handle() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        fs::create_dir(&root).unwrap();
        let image = root.join("image.png");
        let pdf = root.join("document.pdf");
        fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
        fs::write(&pdf, b"%PDF-original").unwrap();
        let image_for_hook = image.clone();
        let pdf_for_hook = pdf.clone();
        let _hook = crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(
            move |opened| {
                if opened == image_for_hook {
                    fs::write(&image_for_hook, b"replacement-image-text").unwrap();
                } else if opened == pdf_for_hook {
                    fs::write(&pdf_for_hook, b"replacement-pdf-text").unwrap();
                }
            },
        ));
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let image_response = read_file_with_scope(single(&image), &scope);
        assert!(!image_response.is_error, "{image_response:?}");
        assert!(matches!(
            image_response.content[0],
            ToolContent::Image { .. }
        ));

        let pdf_response = read_file_with_scope(single(&pdf), &scope);
        let pdf_text = response_text(pdf_response);
        assert!(!pdf_text.contains("replacement-pdf-text"), "{pdf_text}");
    }

    #[test]
    fn scoped_read_denies_outside_file() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("work");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::write(&outside, b"denied-content").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        let response = read_file_with_scope(single(&outside), &scope);
        assert!(response.is_error, "{response:?}");
        let text = response_text(response);
        assert!(text.starts_with("Permission denied: "), "{text}");
        assert!(!text.contains("denied-content"), "{text}");
    }

    #[test]
    fn scoped_batch_keeps_allowed_entries_and_reports_denied_entries_inline() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        let inside = root.join("inside.txt");
        fs::write(&inside, b"allowed-batch-content").unwrap();
        fs::write(&outside, b"denied-batch-content").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = batch([text_entry(&inside), text_entry(&outside)]);
        let response = read_file_with_scope(request, &scope);
        assert!(!response.is_error, "{response:?}");
        let text = response_text(response);
        assert!(text.contains("allowed-batch-content"), "{text}");
        assert!(text.contains("Permission denied: "), "{text}");
        assert!(!text.contains("denied-batch-content"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn direct_read_pins_open_handle_before_final_component_swap_in_both_modes() {
        for restricted in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("allowed");
            fs::create_dir(&root).unwrap();
            let path = root.join("inside.txt");
            let old_path = root.join("inside.old");
            fs::write(&path, b"original-inside-bytes").unwrap();
            let _hook = replace_file_after_open(&path, &old_path, b"replacement-outside-bytes");
            let scope = scope_for(&root, restricted);
            let response = read_file_with_scope(single(&path), &scope);
            let text = response_text(response);
            assert!(
                text.contains("original-inside-bytes"),
                "{restricted}: {text}"
            );
            assert!(
                !text.contains("replacement-outside-bytes"),
                "{restricted}: {text}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn direct_read_pins_open_handle_before_windows_rename_swap_in_both_modes() {
        use std::sync::atomic::{AtomicBool, Ordering};

        for restricted in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("allowed");
            fs::create_dir(&root).unwrap();
            let path = root.join("inside.txt");
            let old_path = root.join("inside.old");
            fs::write(&path, b"original-windows-bytes").unwrap();
            let renamed = Arc::new(AtomicBool::new(false));
            let renamed_for_hook = Arc::clone(&renamed);
            let path_for_hook = path.clone();
            let old_for_hook = old_path.clone();
            let _hook = crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(
                move |_| {
                    if !renamed_for_hook.swap(true, Ordering::AcqRel) {
                        fs::rename(&path_for_hook, &old_for_hook)
                            .expect("capability handle must allow a share-delete rename");
                        fs::write(&path_for_hook, b"replacement-windows-bytes").unwrap();
                    }
                },
            ));
            let scope = scope_for(&root, restricted);
            let text = response_text(read_file_with_scope(single(&path), &scope));
            assert!(renamed.load(Ordering::Acquire));
            assert!(
                text.contains("original-windows-bytes"),
                "{restricted}: {text}"
            );
            assert!(
                !text.contains("replacement-windows-bytes"),
                "{restricted}: {text}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn batch_pins_open_handle_for_swapped_entry_in_both_modes_and_keeps_neighbor() {
        for restricted in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("allowed");
            fs::create_dir(&root).unwrap();
            let first = root.join("first.txt");
            let second = root.join("second.txt");
            let old = root.join("first.old");
            fs::write(&first, b"original-first").unwrap();
            fs::write(&second, b"stable-second").unwrap();
            let _hook = replace_file_after_open(&first, &old, b"replacement-first");
            let scope = scope_for(&root, restricted);
            let request = batch([text_entry(&first), text_entry(&second)]);
            let text = response_text(read_file_with_scope(request, &scope));
            assert!(text.contains("original-first"), "{restricted}: {text}");
            assert!(text.contains("stable-second"), "{restricted}: {text}");
            assert!(!text.contains("replacement-first"), "{restricted}: {text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn direct_read_pins_open_handle_before_ancestor_swap_in_both_modes() {
        for restricted in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("allowed");
            let moved = fixture.path().join("allowed.old");
            fs::create_dir(&root).unwrap();
            let path = root.join("inside.txt");
            fs::write(&path, b"original-ancestor-bytes").unwrap();
            let root_for_hook = root.clone();
            let moved_for_hook = moved.clone();
            let _hook = crate::file_snapshot::tests::OriginalOpenObserverGuard::install(Arc::new(
                move |_| {
                    if root_for_hook.exists() {
                        fs::rename(&root_for_hook, &moved_for_hook).unwrap();
                        fs::create_dir(&root_for_hook).unwrap();
                        fs::write(root_for_hook.join("inside.txt"), b"replacement-ancestor")
                            .unwrap();
                    }
                },
            ));
            let scope = scope_for(&root, restricted);
            let text = response_text(read_file_with_scope(single(&path), &scope));
            assert!(
                text.contains("original-ancestor-bytes"),
                "{restricted}: {text}"
            );
            assert!(
                !text.contains("replacement-ancestor"),
                "{restricted}: {text}"
            );
        }
    }

    #[test]
    fn restricted_relative_single_and_batch_requests_use_stable_absolute_errors() {
        let fixture = tempfile::tempdir().unwrap();
        let scope = ReadScope::from_allow_roots(&[fixture.path().to_path_buf()]).unwrap();
        let single_response =
            read_file_with_scope(single_input("relative.txt".to_string()), &scope);
        assert_eq!(
            response_text(single_response),
            "Path must be absolute: relative.txt"
        );

        let batch_response =
            read_file_with_scope(batch([batch_entry("relative.txt".to_string())]), &scope);
        let text = response_text(batch_response);
        assert!(
            text.contains("Path must be absolute: relative.txt"),
            "{text}"
        );
        assert!(!text.contains("session working directory"), "{text}");
    }

    #[test]
    fn restricted_batch_denied_aliases_remain_inline_instead_of_duplicate_errors() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::write(&outside, b"must-not-leak").unwrap();
        let alias = root.join("..").join("outside.txt");
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = batch([text_entry(&outside), text_entry(&alias)]);
        let response = read_file_with_scope(request, &scope);
        assert!(!response.is_error, "{response:?}");
        let text = response_text(response);
        assert_eq!(text.matches("Permission denied: ").count(), 2, "{text}");
        assert!(!text.contains("must-not-leak"), "{text}");
        assert!(!text.contains("Duplicate path"), "{text}");
    }
}
