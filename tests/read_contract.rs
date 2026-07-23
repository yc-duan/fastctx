mod common;

use common::{cwd, error_text, normalized, text, write};
use fastctx::ToolContent;
use fastctx::read_tool::{BatchReadEntry, ReadRequest, read_file};
use std::fs;

type RequestMutator = fn(&mut ReadRequest);

fn request(path: &std::path::Path) -> ReadRequest {
    ReadRequest {
        file_path: Some(normalized(path)),
        files: None,
        offset: None,
        limit: None,
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    }
}

fn batch_request(files: Vec<BatchReadEntry>) -> ReadRequest {
    ReadRequest {
        file_path: None,
        files: Some(files),
        offset: None,
        limit: None,
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    }
}

fn batch_entry(path: &std::path::Path) -> BatchReadEntry {
    BatchReadEntry {
        path: normalized(path),
        offset: None,
        limit: None,
        encoding: None,
    }
}

#[test]
fn batch_read_preserves_request_order_and_scopes_transcoding_notes() {
    let temp = tempfile::tempdir().unwrap();
    let second = temp.path().join("second.txt");
    let first = temp.path().join("first-utf16.txt");
    write(&second, b"zeta\nend");
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "alpha\n中文".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    write(&first, utf16);

    assert_eq!(
        text(read_file(batch_request(vec![
            batch_entry(&second),
            batch_entry(&first),
        ]))),
        format!(
            "=== {} (lines 1-2 of 2) ===\n1\tzeta\n2\tend\n\n=== {} (lines 1-2 of 2) ===\n(Note: decoded from UTF-16LE; output is UTF-8.)\n1\talpha\n2\t中文\n\n(Complete: 2 files processed.)",
            normalized(&second),
            normalized(&first)
        )
    );
}

#[test]
fn batch_read_continues_after_an_explicit_page_and_returns_an_exact_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let paged = temp.path().join("paged.txt");
    let tail = temp.path().join("tail.txt");
    write(&paged, b"one\ntwo\nthree");
    write(&tail, b"tail");
    let mut first = batch_entry(&paged);
    first.limit = Some(2);

    assert_eq!(
        text(read_file(batch_request(vec![first, batch_entry(&tail)]))),
        format!(
            "=== {} (lines 1-2 of 3) ===\n1\tone\n2\ttwo\n\n=== {} (lines 1-1 of 1) ===\n1\ttail\n\n(Partial: 1 of 2 files processed. Continue with files=[{{\"path\":\"{}\",\"offset\":3}}].)",
            normalized(&paged),
            normalized(&tail),
            normalized(&paged)
        )
    );

    let mut continuation = batch_entry(&paged);
    continuation.offset = Some(3);
    assert_eq!(
        text(read_file(batch_request(vec![continuation]))),
        format!(
            "=== {} (lines 3-3 of 3) ===\n3\tthree\n\n(Complete: 1 file processed.)",
            normalized(&paged)
        )
    );
}

#[test]
fn batch_read_reports_file_failures_inline_without_discarding_neighbors() {
    let temp = tempfile::tempdir().unwrap();
    let valid = temp.path().join("valid.txt");
    let missing = temp.path().join("absent-9f81e043.txt");
    let directory = temp.path().join("directory");
    let binary = temp.path().join("binary.dat");
    let ambiguous = temp.path().join("ambiguous.dat");
    let mixed = temp.path().join("mixed.dat");
    let empty = temp.path().join("empty.txt");
    let short = temp.path().join("short.txt");
    let pdf = temp.path().join("document.pdf");
    let image = temp.path().join("image.bin");
    write(&valid, b"valid");
    fs::create_dir(&directory).unwrap();
    write(&binary, b"prefix\0payload");
    write(&ambiguous, [0xA1, 0xA1]);
    let utf8_prefix = "UTF-8 前缀内容足够清晰，包含多字节字符。\n";
    let mut mixed_bytes = utf8_prefix.as_bytes().to_vec();
    mixed_bytes.extend(
        hex::decode("d6d0cec4cbd1cbf7b1e0c2ebd1e9d6a4cec4b1bed7e3b9bbb3a40ab5dab6fed0d0bcccd0f8b0fcbaacb8fcb6e0d6d0cec4d7d6b7fb")
            .unwrap(),
    );
    write(&mixed, mixed_bytes);
    write(&empty, []);
    write(&short, b"only");
    write(&pdf, b"%PDF-1.7\n");
    write(&image, b"\x89PNG\r\n\x1A\n");
    let mut beyond = batch_entry(&short);
    beyond.offset = Some(2);

    let response = read_file(batch_request(vec![
        batch_entry(&valid),
        batch_entry(&missing),
        batch_entry(&directory),
        batch_entry(&binary),
        batch_entry(&ambiguous),
        batch_entry(&mixed),
        batch_entry(&empty),
        beyond,
        batch_entry(&pdf),
        batch_entry(&image),
    ]));
    assert!(!response.is_error);
    assert_eq!(
        text(response),
        format!(
            "=== {valid} (lines 1-1 of 1) ===\n1\tvalid\n\n=== {missing} ===\nFile does not exist: {missing}\nNote: the session working directory is {cwd}.\n\n=== {directory} ===\n{directory} is a directory, not a file. Use the glob tool to list its contents.\n\n=== {binary} ===\nCannot read binary file as text: {binary}. Use view=\"hex\" to inspect its raw bytes.\n\n=== {ambiguous} ===\nCannot determine the text encoding of {ambiguous} with confidence: the bytes decode cleanly as windows-1252, gbk, shift_jis, big5, euc-kr. Retry with encoding=\"...\" if the context tells you which one, or use view=\"hex\".\n\n=== {mixed} ===\nCannot decode {mixed} as text: it appears to contain mixed or inconsistent encodings — no single encoding explains the whole file. The first conflicting bytes are at hex-view offset {conflict_offset}. Use view=\"hex\" to inspect the raw bytes, or split/normalize the file to a single encoding externally.\n\n=== {empty} ===\nWarning: the file exists but is empty.\n\n=== {short} ===\nWarning: the file has only 1 line, but offset=2 was requested.\n\n=== {pdf} ===\nPDF files cannot be included in files. Read this file separately with file_path and optional pages/pdf_mode.\n\n=== {image} ===\nImage files cannot be included in files. Read this file separately with file_path.\n\n(Complete: 10 files processed.)",
            valid = normalized(&valid),
            missing = normalized(&missing),
            cwd = cwd(),
            directory = normalized(&directory),
            binary = normalized(&binary),
            ambiguous = normalized(&ambiguous),
            mixed = normalized(&mixed),
            conflict_offset = utf8_prefix.len() / 16 + 1,
            empty = normalized(&empty),
            short = normalized(&short),
            pdf = normalized(&pdf),
            image = normalized(&image),
        )
    );
}

#[test]
fn batch_read_rejects_every_request_shape_error_before_file_processing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("valid.txt");
    write(&path, b"valid");

    let mut both = request(&path);
    both.files = Some(vec![batch_entry(&path)]);
    assert_eq!(
        error_text(read_file(both)),
        "Provide exactly one of file_path or files."
    );
    let mut neither = batch_request(vec![batch_entry(&path)]);
    neither.files = None;
    assert_eq!(
        error_text(read_file(neither)),
        "Provide exactly one of file_path or files."
    );
    assert_eq!(
        error_text(read_file(batch_request(Vec::new()))),
        "Invalid files value: expected 1 to 32 entries, got 0."
    );
    assert_eq!(
        error_text(read_file(batch_request(
            (0..33).map(|_| batch_entry(&path)).collect()
        ))),
        "Invalid files value: expected 1 to 32 entries, got 33."
    );

    let single_file_parameter_cases: [(&str, RequestMutator, &str); 3] = [
        (
            "offset",
            |request: &mut ReadRequest| request.offset = Some(1),
            "The top-level offset parameter cannot be combined with files; set it inside the files entries instead.",
        ),
        (
            "limit",
            |request: &mut ReadRequest| request.limit = Some(1),
            "The top-level limit parameter cannot be combined with files; set it inside the files entries instead.",
        ),
        (
            "encoding",
            |request: &mut ReadRequest| request.encoding = Some("utf-8".to_string()),
            "The top-level encoding parameter cannot be combined with files; set it inside the files entries instead.",
        ),
    ];
    for (parameter, mutate, expected) in single_file_parameter_cases {
        let mut invalid = batch_request(vec![batch_entry(&path)]);
        mutate(&mut invalid);
        assert_eq!(error_text(read_file(invalid)), expected, "{parameter}");
    }
    let channel_parameter_cases: [(&str, RequestMutator); 3] = [
        ("pages", |request: &mut ReadRequest| {
            request.pages = Some("1".to_string())
        }),
        ("pdf_mode", |request: &mut ReadRequest| {
            request.pdf_mode = Some("text".to_string())
        }),
        ("view", |request: &mut ReadRequest| {
            request.view = Some("auto".to_string())
        }),
    ];
    for (parameter, mutate) in channel_parameter_cases {
        let mut invalid = batch_request(vec![batch_entry(&path)]);
        mutate(&mut invalid);
        assert_eq!(
            error_text(read_file(invalid)),
            format!(
                "The {parameter} parameter cannot be combined with files; PDFs, images, and hex view are single-file reads."
            )
        );
    }

    let mut zero_offset = batch_entry(&path);
    zero_offset.offset = Some(0);
    assert_eq!(
        error_text(read_file(batch_request(vec![zero_offset]))),
        "Invalid offset value: 0. Expected an integer >= 1."
    );
    let mut zero_limit = batch_entry(&path);
    zero_limit.limit = Some(0);
    assert_eq!(
        error_text(read_file(batch_request(vec![zero_limit]))),
        "Invalid limit value: 0. Expected an integer >= 1."
    );
    let mut invalid_encoding = batch_entry(&path);
    invalid_encoding.encoding = Some("not-an-encoding".to_string());
    assert_eq!(
        error_text(read_file(batch_request(vec![invalid_encoding]))),
        "Invalid encoding value \"not-an-encoding\". Use a WHATWG encoding label such as \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", \"windows-1252\", \"utf-16le\", or \"utf-32le\"."
    );

    let duplicate = BatchReadEntry {
        path: normalized(&path).replace('/', "\\"),
        ..batch_entry(&path)
    };
    assert_eq!(
        error_text(read_file(batch_request(vec![
            batch_entry(&path),
            duplicate
        ]))),
        format!(
            "Duplicate path in files: {}. List each file once.",
            normalized(&path)
        )
    );
}

#[test]
fn batch_read_reports_a_relative_entry_inline_without_discarding_neighbors() {
    let temp = tempfile::tempdir().unwrap();
    let valid = temp.path().join("valid.txt");
    write(&valid, b"valid");
    let relative = "batch-relative-9f81e043.txt";
    let response = read_file(batch_request(vec![
        BatchReadEntry {
            path: relative.to_string(),
            offset: None,
            limit: None,
            encoding: None,
        },
        batch_entry(&valid),
    ]));

    assert!(!response.is_error);
    assert_eq!(
        text(response),
        format!(
            "=== {relative} ===\nFile does not exist: {relative}\nNote: the session working directory is {cwd}.\n\n=== {valid} (lines 1-1 of 1) ===\n1\tvalid\n\n(Complete: 2 files processed.)",
            cwd = cwd(),
            valid = normalized(&valid),
        )
    );
}

#[test]
fn batch_read_accepts_the_full_thirty_two_entry_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let entries = (0..32)
        .map(|index| {
            let path = temp.path().join(format!("file-{index:02}.txt"));
            write(&path, format!("value-{index:02}").as_bytes());
            batch_entry(&path)
        })
        .collect::<Vec<_>>();
    let output = text(read_file(batch_request(entries)));
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("=== "))
            .count(),
        32
    );
    assert!(output.ends_with("(Complete: 32 files processed.)"));
}

#[cfg(windows)]
#[test]
fn batch_read_duplicate_detection_normalizes_windows_drive_case() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("drive-case.txt");
    write(&path, b"value");
    let canonical = normalized(&path);
    let mut lower_drive = canonical.clone();
    lower_drive.replace_range(0..1, &canonical[..1].to_ascii_lowercase());
    let duplicate = BatchReadEntry {
        path: lower_drive.clone(),
        ..batch_entry(&path)
    };
    assert_eq!(
        error_text(read_file(batch_request(vec![
            batch_entry(&path),
            duplicate
        ]))),
        format!("Duplicate path in files: {lower_drive}. List each file once.")
    );
}

#[test]
fn read_numbers_lines_normalizes_crlf_and_preserves_trailing_empty_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sample.txt");
    write(&path, b"one\r\ntwo\r\n");

    assert_eq!(
        text(read_file(request(&path))),
        "1\tone\n2\ttwo\n3\t\n\n(Complete: reached end of file; lines 1-3 of 3 shown.)"
    );
}

#[test]
fn read_lf_and_crlf_files_have_byte_identical_output() {
    let temp = tempfile::tempdir().unwrap();
    let lf = temp.path().join("lf.txt");
    let crlf = temp.path().join("crlf.txt");
    write(&lf, b"one\ntwo\n");
    write(&crlf, b"one\r\ntwo\r\n");
    assert_eq!(
        text(read_file(request(&lf))),
        text(read_file(request(&crlf)))
    );
}

#[test]
fn read_offset_limit_has_exact_continuation_note() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sample.txt");
    write(&path, b"one\ntwo\nthree");
    let response = read_file(ReadRequest {
        file_path: Some(normalized(&path)),
        files: None,
        offset: Some(2),
        limit: Some(1),
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    });

    assert_eq!(
        text(response),
        "2\ttwo\n\n(Partial: line 2 of 3 shown. Continue with offset=3.)"
    );
}

#[test]
fn read_offset_page_that_reaches_eof_uses_a_complete_terminal_note() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tail.txt");
    write(&path, b"one\ntwo\nthree");
    assert_eq!(
        text(read_file(ReadRequest {
            file_path: Some(normalized(&path)),
            files: None,
            offset: Some(2),
            limit: Some(10),
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "2\ttwo\n3\tthree\n\n(Complete: reached end of file; lines 2-3 of 3 shown.)"
    );
}

#[test]
fn read_default_line_limit_distinguishes_two_thousand_from_two_thousand_one() {
    let temp = tempfile::tempdir().unwrap();
    let exact = temp.path().join("exact.txt");
    write(&exact, "\n".repeat(1_999));
    let exact_output = text(read_file(request(&exact)));
    assert_eq!(
        exact_output
            .lines()
            .filter(|line| line.contains('\t'))
            .count(),
        2_000
    );
    assert!(exact_output.ends_with("(Complete: reached end of file; lines 1-2000 of 2000 shown.)"));

    let over = temp.path().join("over.txt");
    write(&over, "\n".repeat(2_000));
    let over_output = text(read_file(request(&over)));
    assert_eq!(
        over_output
            .lines()
            .filter(|line| line.contains('\t'))
            .count(),
        2_000
    );
    assert!(
        over_output.ends_with("(Partial: lines 1-2000 of 2001 shown. Continue with offset=2001.)")
    );
}

#[test]
fn read_empty_offset_and_directory_paths_use_contract_messages() {
    let temp = tempfile::tempdir().unwrap();
    let empty = temp.path().join("empty.txt");
    write(&empty, []);
    assert_eq!(
        text(read_file(request(&empty))),
        "Warning: the file exists but is empty."
    );
    let bom_only = temp.path().join("bom-only.txt");
    write(&bom_only, [0xEF, 0xBB, 0xBF]);
    assert_eq!(
        text(read_file(request(&bom_only))),
        "Warning: the file exists but is empty."
    );

    let short = temp.path().join("short.txt");
    write(&short, b"only");
    assert_eq!(
        text(read_file(ReadRequest {
            file_path: Some(normalized(&short)),
            files: None,
            offset: Some(2),
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "Warning: the file has only 1 line, but offset=2 was requested."
    );

    let two_lines = temp.path().join("two-lines.txt");
    write(&two_lines, b"one\ntwo");
    assert_eq!(
        text(read_file(ReadRequest {
            file_path: Some(normalized(&two_lines)),
            files: None,
            offset: Some(3),
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "Warning: the file has only 2 lines, but offset=3 was requested."
    );

    assert_eq!(
        error_text(read_file(request(temp.path()))),
        format!(
            "{} is a directory, not a file. Use the glob tool to list its contents.",
            normalized(temp.path())
        )
    );
}

#[test]
fn read_rejects_binary_and_non_pdf_pages() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("binary.dat");
    write(&binary, b"prefix\0payload");
    assert_eq!(
        error_text(read_file(request(&binary))),
        format!(
            "Cannot read binary file as text: {}. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&binary)
        )
    );

    let text_path = temp.path().join("sample.txt");
    write(&text_path, b"text");
    assert_eq!(
        error_text(read_file(ReadRequest {
            file_path: Some(normalized(&text_path)),
            files: None,
            offset: None,
            limit: None,
            pages: Some("1".to_string()),
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "The pages parameter only applies to PDF files."
    );
    assert_eq!(
        error_text(read_file(ReadRequest {
            file_path: Some(normalized(&text_path)),
            files: None,
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: Some("image".to_string()),
            encoding: None,
            view: None,
        })),
        "The pdf_mode parameter only applies to PDF files."
    );
}

#[test]
fn read_decodes_utf16_and_gbk_with_declared_source_encoding() {
    let temp = tempfile::tempdir().unwrap();
    let utf16 = temp.path().join("utf16.txt");
    let mut utf16_bytes = vec![0xFF, 0xFE];
    for unit in "alpha\n中文".encode_utf16() {
        utf16_bytes.extend(unit.to_le_bytes());
    }
    write(&utf16, utf16_bytes);
    assert_eq!(
        text(read_file(request(&utf16))),
        "1\talpha\n2\t中文\n\n(Note: decoded from UTF-16LE; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );

    let gbk = temp.path().join("gbk.txt");
    write(
        &gbk,
        hex::decode("d6d0cec4cbd1cbf7b1e0c2ebd1e9d6a4cec4b1bed7e3b9bbb3a40ab5dab6fed0d0bcccd0f8b0fcbaacb8fcb6e0d6d0cec4d7d6b7fb")
            .unwrap(),
    );
    assert_eq!(
        text(read_file(request(&gbk))),
        "1\t中文搜索编码验证文本足够长\n2\t第二行继续包含更多中文字符\n\n(Note: decoded from GBK; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );

    let utf16be = temp.path().join("utf16be.txt");
    let mut utf16be_bytes = vec![0xFE, 0xFF];
    for unit in "big\nendian".encode_utf16() {
        utf16be_bytes.extend(unit.to_be_bytes());
    }
    write(&utf16be, utf16be_bytes);
    assert_eq!(
        text(read_file(request(&utf16be))),
        "1\tbig\n2\tendian\n\n(Note: decoded from UTF-16BE; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );
}

#[test]
fn malformed_utf8_with_low_legacy_evidence_is_rejected_as_ambiguous() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("malformed-utf8.txt");
    write(&path, b"valid\xFFtail");
    assert_eq!(
        error_text(read_file(request(&path))),
        format!(
            "Cannot determine the text encoding of {} with confidence: the bytes decode cleanly as windows-1252. Retry with encoding=\"...\" if the context tells you which one, or use view=\"hex\".",
            normalized(&path)
        )
    );
}

#[test]
fn read_accepts_utf8_and_utf32_boms_and_rejects_bom_content_mismatches() {
    let temp = tempfile::tempdir().unwrap();

    let utf8 = temp.path().join("utf8-bom.txt");
    let mut utf8_bytes = b"\xEF\xBB\xBF".to_vec();
    utf8_bytes.extend("alpha\n中文".as_bytes());
    write(&utf8, utf8_bytes);
    assert_eq!(
        text(read_file(request(&utf8))),
        "1\talpha\n2\t中文\n\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );

    for (name, little_endian, label) in [
        ("utf32le.txt", true, "UTF-32LE"),
        ("utf32be.txt", false, "UTF-32BE"),
    ] {
        let path = temp.path().join(name);
        write(&path, utf32_bytes("wide\n字符", little_endian, true));
        assert_eq!(
            text(read_file(request(&path))),
            format!(
                "1\twide\n2\t字符\n\n(Note: decoded from {label}; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
            )
        );
    }

    let malformed_utf8 = temp.path().join("bad-utf8-bom.txt");
    write(&malformed_utf8, b"\xEF\xBB\xBFvalid\xFF");
    assert_eq!(
        error_text(read_file(request(&malformed_utf8))),
        format!(
            "Cannot decode {}: it has a UTF-8 byte order mark but the content is not valid UTF-8. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&malformed_utf8)
        )
    );

    let malformed_utf32 = temp.path().join("bad-utf32-bom.txt");
    let mut bytes = vec![0xFF, 0xFE, 0x00, 0x00];
    bytes.extend(0x11_0000_u32.to_le_bytes());
    write(&malformed_utf32, bytes);
    assert_eq!(
        error_text(read_file(request(&malformed_utf32))),
        format!(
            "Cannot decode {}: it has a UTF-32LE byte order mark but the content is not valid UTF-32LE. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&malformed_utf32)
        )
    );
}

#[test]
fn explicit_encoding_overrides_the_nul_gate_and_never_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let utf16 = temp.path().join("no-bom-utf16le.txt");
    let mut utf16_bytes = Vec::new();
    for unit in "plain\n中文".encode_utf16() {
        utf16_bytes.extend(unit.to_le_bytes());
    }
    write(&utf16, utf16_bytes);
    assert_eq!(
        error_text(read_file(request(&utf16))),
        format!(
            "Cannot read binary file as text: {}. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&utf16)
        )
    );
    let mut explicit = request(&utf16);
    explicit.encoding = Some("utf-16le".to_string());
    assert_eq!(
        text(read_file(explicit)),
        "1\tplain\n2\t中文\n\n(Note: decoded from UTF-16LE as requested; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );

    let utf16be = temp.path().join("no-bom-utf16be.txt");
    let mut utf16be_bytes = Vec::new();
    for unit in "big\nendian".encode_utf16() {
        utf16be_bytes.extend(unit.to_be_bytes());
    }
    write(&utf16be, utf16be_bytes);
    let mut explicit = request(&utf16be);
    explicit.encoding = Some("utf-16be".to_string());
    assert_eq!(
        text(read_file(explicit)),
        "1\tbig\n2\tendian\n\n(Note: decoded from UTF-16BE as requested; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );

    for (name, little_endian, encoding, label) in [
        ("no-bom-utf32le.txt", true, "utf-32le", "UTF-32LE"),
        ("no-bom-utf32be.txt", false, "utf-32be", "UTF-32BE"),
    ] {
        let utf32 = temp.path().join(name);
        write(&utf32, utf32_bytes("wide\ntext", little_endian, false));
        let mut explicit = request(&utf32);
        explicit.encoding = Some(encoding.to_string());
        assert_eq!(
            text(read_file(explicit)),
            format!(
                "1\twide\n2\ttext\n\n(Note: decoded from {label} as requested; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
            )
        );
    }

    let short_gbk = temp.path().join("short-gbk.txt");
    write(&short_gbk, hex::decode("d6d0cec4").unwrap());
    let mut explicit = request(&short_gbk);
    explicit.encoding = Some("gbk".to_string());
    assert_eq!(
        text(read_file(explicit)),
        "1\t中文\n\n(Note: decoded from GBK as requested; output is UTF-8.)\n(Complete: reached end of file; line 1 of 1 shown.)"
    );

    let invalid = temp.path().join("invalid.txt");
    write(&invalid, [0x81]);
    let mut explicit = request(&invalid);
    explicit.encoding = Some("gbk".to_string());
    assert_eq!(
        error_text(read_file(explicit)),
        format!(
            "Cannot decode {} as gbk: the content is not valid gbk. Try another encoding or view=\"hex\".",
            normalized(&invalid)
        )
    );

    let malformed_utf16 = temp.path().join("invalid-utf16be.txt");
    write(&malformed_utf16, [0x00]);
    let mut explicit = request(&malformed_utf16);
    explicit.encoding = Some("utf-16be".to_string());
    assert_eq!(
        error_text(read_file(explicit)),
        format!(
            "Cannot decode {} as utf-16be: the content is not valid utf-16be. Try another encoding or view=\"hex\".",
            normalized(&malformed_utf16)
        )
    );
}

#[test]
fn encoding_errors_list_fixed_candidates_and_validate_channel_applicability() {
    let temp = tempfile::tempdir().unwrap();
    let ambiguous = temp.path().join("ambiguous.txt");
    write(&ambiguous, [0xA1, 0xA1]);
    assert_eq!(
        error_text(read_file(request(&ambiguous))),
        format!(
            "Cannot determine the text encoding of {} with confidence: the bytes decode cleanly as windows-1252, gbk, shift_jis, big5, euc-kr. Retry with encoding=\"...\" if the context tells you which one, or use view=\"hex\".",
            normalized(&ambiguous)
        )
    );

    let undecodable = temp.path().join("undecodable.txt");
    write(&undecodable, [0x81]);
    assert_eq!(
        error_text(read_file(request(&undecodable))),
        format!(
            "Cannot decode {} as text: no supported encoding decodes it cleanly. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&undecodable)
        )
    );

    let empty = temp.path().join("empty.txt");
    write(&empty, []);
    let mut invalid_label = request(&empty);
    invalid_label.encoding = Some("definitely-not-an-encoding".to_string());
    assert_eq!(
        error_text(read_file(invalid_label)),
        "Invalid encoding value \"definitely-not-an-encoding\". Use a WHATWG encoding label such as \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", \"windows-1252\", \"utf-16le\", or \"utf-32le\"."
    );

    let image = temp.path().join("image.bin");
    write(&image, b"\x89PNG\r\n\x1A\n");
    let mut image_request = request(&image);
    image_request.encoding = Some("gbk".to_string());
    assert_eq!(
        error_text(read_file(image_request)),
        "The encoding parameter only applies to text files."
    );

    let pdf = temp.path().join("document.bin");
    write(&pdf, b"%PDF-1.7\n");
    let mut pdf_request = request(&pdf);
    pdf_request.encoding = Some("gbk".to_string());
    assert_eq!(
        error_text(read_file(pdf_request)),
        "The encoding parameter only applies to text files."
    );
}

#[test]
fn dense_shift_jis_is_trusted_but_legal_utf8_is_never_second_guessed() {
    let temp = tempfile::tempdir().unwrap();
    let shift_jis = temp.path().join("shift-jis.txt");
    let source = "日本語の文字列を十分な長さまで繰り返して検出を安定させます。日本語検索です。";
    write(
        &shift_jis,
        hex::decode("93fa967b8cea82cc95b68e9a97f182f08f5c95aa82c892b782b382dc82c58c4a82e895d482b582c48c9f8f6f82f088c092e882b382b982dc82b7814293fa967b8cea8c9f8df582c582b78142")
            .unwrap(),
    );
    assert_eq!(
        text(read_file(request(&shift_jis))),
        format!(
            "1\t{source}\n\n(Note: decoded from Shift_JIS; output is UTF-8.)\n(Complete: reached end of file; line 1 of 1 shown.)"
        )
    );

    let utf8 = temp.path().join("mojibake-discussion.txt");
    write(
        &utf8,
        "validÿtail and mojibake examples: ä¸\u{AD}æ–‡".as_bytes(),
    );
    assert_eq!(
        text(read_file(request(&utf8))),
        "1\tvalidÿtail and mojibake examples: ä¸\u{AD}æ–‡\n\n(Complete: reached end of file; line 1 of 1 shown.)"
    );

    let magic_like_utf8 = temp.path().join("magic-like.txt");
    write(
        &magic_like_utf8,
        b"MZ is discussed here as plain UTF-8 text",
    );
    assert_eq!(
        text(read_file(request(&magic_like_utf8))),
        "1\tMZ is discussed here as plain UTF-8 text\n\n(Complete: reached end of file; line 1 of 1 shown.)"
    );
}

#[test]
fn automatic_legacy_detection_rejects_single_byte_and_mixed_files_without_poisoning_gbk() {
    let temp = tempfile::tempdir().unwrap();

    let windows_1252 = temp.path().join("windows-1252.txt");
    let mut windows_1252_bytes = Vec::new();
    for _ in 0..8 {
        windows_1252_bytes.extend([0x93, b'A', 0x94, b' ', 0x96, b' ', 0xE9, b' ']);
    }
    write(&windows_1252, windows_1252_bytes);
    let windows_1252_error = error_text(read_file(request(&windows_1252)));
    assert!(
        windows_1252_error.starts_with(&format!(
            "Cannot determine the text encoding of {} with confidence:",
            normalized(&windows_1252)
        )),
        "{windows_1252_error}"
    );
    assert!(windows_1252_error.contains("windows-1252"));

    let gbk_bytes = hex::decode("d6d0cec4cbd1cbf7b1e0c2ebd1e9d6a4cec4b1bed7e3b9bbb3a40ab5dab6fed0d0bcccd0f8b0fcbaacb8fcb6e0d6d0cec4d7d6b7fb")
        .unwrap();
    let utf8_prefix = "UTF-8 前缀内容足够清晰，包含多字节字符。\n";
    let forward = temp.path().join("utf8-then-gbk.txt");
    let mut forward_bytes = utf8_prefix.as_bytes().to_vec();
    forward_bytes.extend_from_slice(&gbk_bytes);
    write(&forward, forward_bytes);
    let conflict_offset = utf8_prefix.len() / 16 + 1;
    assert_eq!(
        error_text(read_file(request(&forward))),
        format!(
            "Cannot decode {} as text: it appears to contain mixed or inconsistent encodings — no single encoding explains the whole file. The first conflicting bytes are at hex-view offset {conflict_offset}. Use view=\"hex\" to inspect the raw bytes, or split/normalize the file to a single encoding externally.",
            normalized(&forward)
        )
    );

    let reverse = temp.path().join("gbk-then-utf8.txt");
    let mut reverse_bytes = gbk_bytes.clone();
    reverse_bytes.push(b'\n');
    reverse_bytes.extend_from_slice("中文".repeat(12).as_bytes());
    write(&reverse, reverse_bytes);
    assert_eq!(
        error_text(read_file(request(&reverse))),
        format!(
            "Cannot decode {} as text: it appears to contain mixed or inconsistent encodings — no single encoding explains the whole file. Use view=\"hex\" to inspect the raw bytes, or split/normalize the file to a single encoding externally.",
            normalized(&reverse)
        )
    );

    let threshold_reverse = temp.path().join("gbk-then-short-utf8.txt");
    let mut threshold_reverse_bytes = gbk_bytes
        .iter()
        .copied()
        .filter(|byte| *byte != b'\n')
        .collect::<Vec<_>>();
    threshold_reverse_bytes.push(b'\n');
    threshold_reverse_bytes.extend_from_slice(b"ASCII padding keeps this segment long: ");
    threshold_reverse_bytes.extend_from_slice("中文".repeat(4).as_bytes());
    write(&threshold_reverse, threshold_reverse_bytes);
    assert_eq!(
        error_text(read_file(request(&threshold_reverse))),
        format!(
            "Cannot decode {} as text: it appears to contain mixed or inconsistent encodings — no single encoding explains the whole file. Use view=\"hex\" to inspect the raw bytes, or split/normalize the file to a single encoding externally.",
            normalized(&threshold_reverse)
        )
    );

    let pure_gbk = temp.path().join("pure-gbk.txt");
    write(&pure_gbk, gbk_bytes);
    assert_eq!(
        text(read_file(request(&pure_gbk))),
        "1\t中文搜索编码验证文本足够长\n2\t第二行继续包含更多中文字符\n\n(Note: decoded from GBK; output is UTF-8.)\n(Complete: reached end of file; lines 1-2 of 2 shown.)"
    );
}

#[test]
fn iso_2022_signature_is_ambiguous_but_explicit_utf8_is_an_escape_hatch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("iso-2022-like.txt");
    let bytes = b"ASCII prefix \x1B$B$3$s$K$A$O\x1B(B";
    write(&path, bytes);

    assert_eq!(
        error_text(read_file(request(&path))),
        format!(
            "Cannot decode {} as text with confidence: the bytes are valid UTF-8 but contain ISO-2022 escape sequences (a stateful encoding such as ISO-2022-JP). Retry with encoding=\"iso-2022-jp\", or encoding=\"utf-8\" to force the raw UTF-8 reading, or use view=\"hex\".",
            normalized(&path)
        )
    );

    let mut explicit = request(&path);
    explicit.encoding = Some("utf-8".to_string());
    assert_eq!(
        text(read_file(explicit)),
        format!(
            "1\t{}\n\n(Complete: reached end of file; line 1 of 1 shown.)",
            String::from_utf8_lossy(bytes)
        )
    );
}

#[test]
fn explicit_legacy_decoding_warns_when_raw_bytes_are_valid_multibyte_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("valid-utf8.txt");
    write(&path, "中文".repeat(12));
    let mut explicit = request(&path);
    explicit.encoding = Some("gbk".to_string());
    let output = text(read_file(explicit));
    assert!(
        output.contains(
            "(Note: decoded from GBK as requested; output is UTF-8. Warning: the raw bytes are also valid UTF-8 — if this looks garbled, retry with encoding=\"utf-8\" or omit encoding.)"
        ),
        "{output}"
    );
    assert!(output.ends_with("(Complete: reached end of file; line 1 of 1 shown.)"));
}

#[test]
fn read_truncates_a_single_unicode_line_at_2000_characters() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("long.txt");
    write(&path, "界".repeat(2_001));
    assert_eq!(
        text(read_file(request(&path))),
        format!(
            "1\t{}... [line truncated: 2001 chars total]\n\n(Complete: reached end of file; line 1 of 1 shown.)",
            "界".repeat(2_000)
        )
    );
}

#[test]
fn read_orders_transcoding_before_the_exact_continuation_note() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("paged-gbk.txt");
    write(
        &path,
        hex::decode("b5dad2bbd0d0b0fcbaacd7e3b9bbb6e0b5c4d6d0cec4d7d6b7fbd3c3d3dabfc9d0c5bcecb2e20ab5dab6fed0d0bcccd0f8b2b9b3e4d6d0cec4d6a4bedd0ab5dac8fdd0d0cad5ceb2")
            .unwrap(),
    );
    assert_eq!(
        text(read_file(ReadRequest {
            file_path: Some(normalized(&path)),
            files: None,
            offset: Some(1),
            limit: Some(1),
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "1\t第一行包含足够多的中文字符用于可信检测\n\n(Note: decoded from GBK; output is UTF-8.)\n(Partial: line 1 of 3 shown. Continue with offset=2.)"
    );
}

#[test]
fn crlf_at_the_character_limit_does_not_drop_content() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("limit-crlf.txt");
    write(&path, format!("{}\r\n", "x".repeat(2_000)));
    assert_eq!(
        text(read_file(request(&path))),
        format!(
            "1\t{}\n2\t\n\n(Complete: reached end of file; lines 1-2 of 2 shown.)",
            "x".repeat(2_000)
        )
    );
}

#[test]
fn image_magic_bytes_override_extension_and_size_limit_is_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("wrong.jpg");
    write(&image, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let response = read_file(request(&image));
    assert!(!response.is_error);
    assert!(matches!(
        response.content.as_slice(),
        [ToolContent::Image { mime_type, detail: None, .. }] if mime_type == "image/png"
    ));

    let at_limit = temp.path().join("at-limit.bmp");
    let mut at_limit_bytes = vec![0_u8; 8 * 1024 * 1024];
    at_limit_bytes[0..2].copy_from_slice(b"BM");
    write(&at_limit, at_limit_bytes);
    let response = read_file(request(&at_limit));
    assert!(!response.is_error, "{response:?}");
    assert!(matches!(
        response.content.as_slice(),
        [ToolContent::Image { mime_type, detail: None, .. }] if mime_type == "image/bmp"
    ));

    let oversized = temp.path().join("large.bmp");
    let mut bytes = vec![0_u8; 8 * 1024 * 1024 + 1];
    bytes[0..2].copy_from_slice(b"BM");
    write(&oversized, bytes);
    assert_eq!(
        error_text(read_file(request(&oversized))),
        "Image file too large: 8.1 MiB (limit: 8 MiB). Resize or convert it externally."
    );
}

#[test]
fn relative_existing_path_still_requests_the_absolute_path() {
    let response = read_file(ReadRequest {
        file_path: Some("README.md".to_string()),
        files: None,
        offset: None,
        limit: None,
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    });
    let error = error_text(response);
    assert!(
        error
            .starts_with("File does not exist: README.md\nNote: the session working directory is ")
    );
    assert!(error.contains("Use the absolute path"));
}

#[test]
fn read_missing_file_suggests_the_nearest_existing_name() {
    let temp = tempfile::tempdir().unwrap();
    let existing = temp.path().join("important-report.txt");
    write(&existing, b"content");
    let missing = temp.path().join("important-repot.txt");
    assert_eq!(
        error_text(read_file(request(&missing))),
        format!(
            "File does not exist: {}\nNote: the session working directory is {}.\nDid you mean: {}?",
            normalized(&missing),
            cwd(),
            normalized(&existing)
        )
    );
}

#[test]
fn read_missing_absolute_file_without_candidate_has_the_exact_cwd_note() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("nothing-like-this.txt");
    assert_eq!(
        error_text(read_file(request(&missing))),
        format!(
            "File does not exist: {}\nNote: the session working directory is {}.",
            normalized(&missing),
            cwd()
        )
    );
}

#[test]
fn read_missing_lossy_filename_explains_the_u_fffd_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("bad-\u{FFFD}-name.txt");
    assert_eq!(
        error_text(read_file(request(&missing))),
        format!(
            "File does not exist: {}\nNote: the session working directory is {}.\nNote: this path contains U+FFFD (a placeholder for bytes that are not valid text); it looks like the lossy rendering of a filename that cannot be represented as text and cannot be opened by name.",
            normalized(&missing),
            cwd()
        )
    );
}

#[cfg(unix)]
#[test]
fn read_lossy_rendering_of_an_actual_non_utf8_filename_has_the_exact_boundary_error() {
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"bad-\xFF-name.txt".to_vec()));
    // APFS rejects file names that are not valid UTF-8; the fixture is only
    // creatable on filesystems that accept arbitrary bytes, e.g. ext4 (2026-07-16).
    if std::fs::write(&path, b"content").is_err() {
        eprintln!("skipping: this filesystem rejects non-UTF-8 file names");
        return;
    }
    let lossy = normalized(&path);
    assert!(lossy.contains('\u{FFFD}'));
    let mut input = request(&path);
    input.file_path = lossy.clone();
    assert_eq!(
        error_text(read_file(input)),
        format!(
            "File does not exist: {lossy}\nNote: the session working directory is {}.\nNote: this path contains U+FFFD (a placeholder for bytes that are not valid text); it looks like the lossy rendering of a filename that cannot be represented as text and cannot be opened by name.",
            cwd()
        )
    );
}

#[test]
fn read_rejects_zero_offset_and_limit_even_when_called_without_schema_validation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sample.txt");
    write(&path, b"one");
    let mut input = request(&path);
    input.offset = Some(0);
    assert_eq!(
        error_text(read_file(input)),
        "Invalid offset value: 0. Expected an integer >= 1."
    );
    let mut input = request(&path);
    input.limit = Some(0);
    assert_eq!(
        error_text(read_file(input)),
        "Invalid limit value: 0. Expected an integer >= 1."
    );
}

#[test]
fn all_supported_image_magic_signatures_return_the_correct_mime() {
    let temp = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[u8], &str)] = &[
        ("png.bin", b"\x89PNG\r\n\x1a\n", "image/png"),
        ("jpeg.bin", b"\xff\xd8\xff", "image/jpeg"),
        ("gif.bin", b"GIF89a", "image/gif"),
        ("webp.bin", b"RIFF\x04\x00\x00\x00WEBP", "image/webp"),
        ("bmp.bin", b"BM", "image/bmp"),
    ];
    for (name, bytes, expected_mime) in cases {
        let path = temp.path().join(name);
        write(&path, bytes);
        let response = read_file(request(&path));
        assert!(!response.is_error, "{response:?}");
        assert!(matches!(
            response.content.as_slice(),
            [ToolContent::Image { mime_type, detail: None, .. }] if mime_type == expected_mime
        ));
    }

    let extension_fallback = temp.path().join("fallback.png");
    write(&extension_fallback, b"extension fallback");
    let response = read_file(request(&extension_fallback));
    assert!(matches!(
        response.content.as_slice(),
        [ToolContent::Image { mime_type, detail: None, .. }] if mime_type == "image/png"
    ));
}

#[cfg(unix)]
#[test]
fn read_permission_denial_uses_the_contract_error() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("forbidden.txt");
    write(&path, b"secret");
    let original = fs::metadata(&path).unwrap().permissions();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    let response = read_file(request(&path));
    fs::set_permissions(&path, original).unwrap();
    assert_eq!(
        error_text(response),
        format!("Permission denied: {}", normalized(&path))
    );
}

#[cfg(unix)]
#[test]
fn read_rejects_non_regular_device_files_before_opening_them() {
    let path = std::path::Path::new("/dev/null");
    assert_eq!(
        error_text(read_file(request(path))),
        "Cannot read non-regular file: /dev/null. Only regular files are supported."
    );
}

#[cfg(windows)]
#[test]
fn read_windows_exclusive_lock_uses_the_contract_error() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("locked.txt");
    write(&path, b"locked");
    let _lock = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_eq!(
        error_text(read_file(request(&path))),
        format!(
            "Cannot open file (locked by another process): {}",
            normalized(&path)
        )
    );
}

#[test]
fn read_large_file_uses_tail_note_without_total_count() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large.txt");
    let mut file = fs::File::create(&path).unwrap();
    use std::io::Write;
    file.write_all(b"first\n").unwrap();
    file.write_all(&vec![b'a'; 8_185]).unwrap();
    file.write_all(b"\n").unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(file);
    // Sparse extension creates NUL bytes after the first 8 KiB, which the explicit 8 KiB probe ignores.
    let output = text(read_file(ReadRequest {
        file_path: Some(normalized(&path)),
        files: None,
        offset: Some(1),
        limit: Some(1),
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: None,
    }));
    assert_eq!(
        output,
        "1\tfirst\n\n(Partial: line 1 shown. Continue with offset=2.)"
    );
    let mut entry = batch_entry(&path);
    entry.limit = Some(1);
    assert_eq!(
        text(read_file(batch_request(vec![entry]))),
        format!(
            "=== {} (lines 1-1) ===\n1\tfirst\n\n(Partial: 0 of 1 files processed. Continue with files=[{{\"path\":\"{}\",\"offset\":2}}].)",
            normalized(&path),
            normalized(&path)
        )
    );
}

#[test]
fn read_large_sparse_file_at_true_eof_restores_the_total_line_count() {
    use std::io::{Seek, SeekFrom, Write};

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large-eof.txt");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(b"first\n").unwrap();
    file.write_all(&vec![b'a'; 8_185]).unwrap();
    file.write_all(b"\n").unwrap();
    let final_size = 64 * 1024 * 1024 + 10;
    file.set_len(final_size).unwrap();
    file.seek(SeekFrom::Start(final_size - 5)).unwrap();
    file.write_all(b"\nlast").unwrap();
    drop(file);

    assert_eq!(
        text(read_file(ReadRequest {
            file_path: Some(normalized(&path)),
            files: None,
            offset: Some(4),
            limit: Some(10),
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        })),
        "4\tlast\n\n(Complete: reached end of file; line 4 of 4 shown.)"
    );
}

fn utf32_bytes(text: &str, little_endian: bool, with_bom: bool) -> Vec<u8> {
    let mut bytes = if with_bom {
        if little_endian {
            vec![0xFF, 0xFE, 0x00, 0x00]
        } else {
            vec![0x00, 0x00, 0xFE, 0xFF]
        }
    } else {
        Vec::new()
    };
    for character in text.chars() {
        let unit = character as u32;
        bytes.extend(if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}
