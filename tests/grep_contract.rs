mod common;

use common::{cwd, error_text, normalized, set_mtime, text, write};
use fastctx::grep_tool::{GrepRequest, OutputMode, grep_files};

fn request(path: &std::path::Path, pattern: &str, mode: OutputMode) -> GrepRequest {
    GrepRequest {
        pattern: pattern.to_string(),
        path: Some(normalized(path)),
        glob: None,
        file_type: None,
        output_mode: Some(mode),
        case_insensitive: None,
        line_numbers: None,
        only_matching: None,
        before_context: None,
        after_context: None,
        context: None,
        multiline: None,
        head_limit: None,
        offset: None,
        encoding: None,
        fallback_encoding: None,
    }
}

#[test]
fn grep_respects_ignore_searches_hidden_and_excludes_git() {
    let temp = tempfile::tempdir().unwrap();
    fs_create_dir(temp.path().join(".git"));
    write(&temp.path().join(".gitignore"), b"ignored.txt\n");
    write(&temp.path().join("ignored.txt"), b"NEEDLE");
    let hidden = temp.path().join(".hidden.txt");
    write(&hidden, b"NEEDLE");
    write(&temp.path().join(".git/HEAD"), b"NEEDLE");

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "NEEDLE",
            OutputMode::FilesWithMatches
        ))),
        format!("{}\n\n(Complete: all 1 file shown.)", normalized(&hidden))
    );
}

#[test]
fn grep_files_and_count_are_sorted_newest_first() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old.txt");
    let new = temp.path().join("new.txt");
    write(&old, b"hit hit\n");
    write(&new, b"hit\n");
    set_mtime(&old, 1_700_000_001);
    set_mtime(&new, 1_700_000_002);

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "hit",
            OutputMode::FilesWithMatches
        ))),
        format!(
            "{}\n{}\n\n(Complete: all 2 files shown.)",
            normalized(&new),
            normalized(&old)
        )
    );
    assert_eq!(
        text(grep_files(request(temp.path(), "hit", OutputMode::Count))),
        format!(
            "{}:1\n{}:2\n\n(Complete: 3 occurrences across 2 files.)",
            normalized(&new),
            normalized(&old)
        )
    );
}

#[test]
fn grep_mtime_ties_use_path_byte_order() {
    let temp = tempfile::tempdir().unwrap();
    let alpha = temp.path().join("alpha.txt");
    let beta = temp.path().join("beta.txt");
    write(&beta, b"hit");
    write(&alpha, b"hit");
    set_mtime(&alpha, 1_700_000_100);
    set_mtime(&beta, 1_700_000_100);
    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "hit",
            OutputMode::FilesWithMatches
        ))),
        format!(
            "{}\n{}\n\n(Complete: all 2 files shown.)",
            normalized(&alpha),
            normalized(&beta)
        )
    );
}

#[test]
fn grep_content_single_file_uses_line_and_context_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sample.txt");
    write(
        &file,
        b"before\nNEEDLE\nafter\ngap\nfar\nNEEDLE again\nafter again\n",
    );
    let mut input = request(&file, "NEEDLE", OutputMode::Content);
    input.context = Some(1);
    assert_eq!(
        text(grep_files(input)),
        "1-before\n2:NEEDLE\n3-after\n--\n5-far\n6:NEEDLE again\n7-after again\n\n(Complete: all 2 results shown.)"
    );
}

#[test]
fn grep_multiline_normalizes_crlf_and_only_matching_is_stable() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("crlf.txt");
    write(&file, b"one\r\nxxxNEEDLE\r\n");
    let mut multiline = request(&file, r"one\nxxxNEEDLE", OutputMode::Content);
    multiline.multiline = Some(true);
    assert_eq!(
        text(grep_files(multiline)),
        "1:one\n2:xxxNEEDLE\n\n(Complete: all 1 result shown.)"
    );

    let mut multiline_only = request(&file, r"one\nxxxNEEDLE", OutputMode::Content);
    multiline_only.multiline = Some(true);
    multiline_only.only_matching = Some(true);
    assert_eq!(
        text(grep_files(multiline_only)),
        "1:one\\nxxxNEEDLE\n\n(Complete: all 1 result shown.)"
    );

    let matches = temp.path().join("matches.txt");
    write(&matches, b"id=12 id=34\n");
    let mut only = request(&matches, r"\d+", OutputMode::Content);
    only.only_matching = Some(true);
    assert_eq!(
        text(grep_files(only)),
        "1:12\n1:34\n\n(Complete: all 2 results shown.)"
    );
}

#[test]
fn grep_only_matching_pages_by_occurrence_without_duplicates_or_gaps() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("occurrences.txt");
    write(&file, b"START id=12 id=34 END\n");

    let mut first = request(&file, r"\d+", OutputMode::Content);
    first.only_matching = Some(true);
    first.head_limit = Some(1);
    assert_eq!(
        text(grep_files(first)),
        "1:12\n\n(Partial: result 1 shown; more exist. Continue with offset=1.)"
    );

    let mut second = request(&file, r"\d+", OutputMode::Content);
    second.only_matching = Some(true);
    second.line_numbers = Some(false);
    second.head_limit = Some(1);
    second.offset = Some(1);
    assert_eq!(
        text(grep_files(second)),
        "34\n\n(Complete: result 2 shown; end of results.)"
    );

    let mut exhausted = request(&file, r"\d+", OutputMode::Content);
    exhausted.only_matching = Some(true);
    exhausted.offset = Some(2);
    assert_eq!(
        text(grep_files(exhausted)),
        "(Complete: no results at offset=2; only 2 results exist.)"
    );
}

#[test]
fn grep_lf_and_crlf_files_have_byte_identical_content_results() {
    let temp = tempfile::tempdir().unwrap();
    let lf = temp.path().join("lf.txt");
    let crlf = temp.path().join("crlf.txt");
    write(&lf, b"before\nhit\nafter\n");
    write(&crlf, b"before\r\nhit\r\nafter\r\n");

    let mut lf_request = request(&lf, "hit", OutputMode::Content);
    lf_request.context = Some(1);
    let mut crlf_request = request(&crlf, "hit", OutputMode::Content);
    crlf_request.context = Some(1);
    assert_eq!(text(grep_files(lf_request)), text(grep_files(crlf_request)));

    let mut lf_multiline = request(&lf, r"before\nhit", OutputMode::Content);
    lf_multiline.multiline = Some(true);
    let mut crlf_multiline = request(&crlf, r"before\nhit", OutputMode::Content);
    crlf_multiline.multiline = Some(true);
    assert_eq!(
        text(grep_files(lf_multiline)),
        text(grep_files(crlf_multiline))
    );
}

#[test]
fn grep_strips_a_utf8_bom_before_matching_and_rendering() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("bom.txt");
    write(&file, b"\xEF\xBB\xBFhit\n");
    assert_eq!(
        text(grep_files(request(&file, "^hit", OutputMode::Content))),
        "1:hit\n\n(Complete: all 1 result shown.)"
    );
}

#[test]
fn grep_line_anchors_are_logical_line_and_crlf_aware_across_encodings() {
    let temp = tempfile::tempdir().unwrap();
    let lf = temp.path().join("lf.txt");
    let crlf = temp.path().join("crlf.txt");
    let utf8_bom = temp.path().join("utf8-bom.txt");
    let utf16le = temp.path().join("utf16le.txt");
    write(&lf, b"alpha\n\nBeta\n");
    write(&crlf, b"alpha\r\n\r\nBeta\r\n");
    write(&utf8_bom, b"\xEF\xBB\xBFalpha\n\nBeta\n");
    let mut utf16le_bytes = Vec::new();
    for unit in "alpha\n\nBeta\n".encode_utf16() {
        utf16le_bytes.extend(unit.to_le_bytes());
    }
    write(&utf16le, utf16le_bytes);

    let cases = [
        ("^alpha$", "1:alpha"),
        ("Beta$", "3:Beta"),
        ("^Beta", "3:Beta"),
        ("^$", "2:"),
    ];
    for (pattern, expected_line) in cases {
        for path in [&lf, &crlf, &utf8_bom] {
            assert_eq!(
                text(grep_files(request(path, pattern, OutputMode::Content))),
                format!("{expected_line}\n\n(Complete: all 1 result shown.)"),
                "pattern {pattern} in {}",
                normalized(path)
            );
        }
        let mut explicit = request(&utf16le, pattern, OutputMode::Content);
        explicit.encoding = Some("utf-16le".to_string());
        assert_eq!(
            text(grep_files(explicit)),
            format!(
                "{expected_line}\n\n(Note: decoded from UTF-16LE as requested; output is UTF-8.)\n(Complete: all 1 result shown.)"
            ),
            "pattern {pattern} in explicit UTF-16LE"
        );
    }

    let mut multiline_empty = request(&crlf, "^$", OutputMode::Content);
    multiline_empty.multiline = Some(true);
    assert_eq!(
        text(grep_files(multiline_empty)),
        "2:\n\n(Complete: all 1 result shown.)"
    );
}

#[test]
fn grep_uses_limit_plus_one_probe_and_exact_offset() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("many.txt");
    let body = (1..=251)
        .map(|index| format!("hit-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    write(&file, body);
    let output = text(grep_files(request(&file, "hit", OutputMode::Content)));
    assert_eq!(
        output.lines().filter(|line| line.contains(":hit-")).count(),
        250
    );
    assert!(
        output.ends_with("(Partial: results 1-250 shown; more exist. Continue with offset=250.)")
    );

    let mut exact = request(&file, "hit", OutputMode::Content);
    exact.head_limit = Some(250);
    exact.offset = Some(1);
    let output = text(grep_files(exact));
    assert!(output.ends_with("(Complete: results 2-251 shown; end of results.)"));
    assert!(output.starts_with("2:hit-2"));
    assert!(output.contains("\n251:hit-251\n\n(Complete:"));

    let exact_file = temp.path().join("exact.txt");
    let exact_body = (1..=250)
        .map(|index| format!("hit-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    write(&exact_file, exact_body);
    let exact_output = text(grep_files(request(&exact_file, "hit", OutputMode::Content)));
    assert_eq!(
        exact_output
            .lines()
            .filter(|line| line.contains(":hit-"))
            .count(),
        250
    );
    assert!(exact_output.ends_with("(Complete: all 250 results shown.)"));
}

#[test]
fn grep_offsets_stream_across_files_without_retaining_skipped_entries() {
    let temp = tempfile::tempdir().unwrap();
    let newest = temp.path().join("newest.txt");
    let middle = temp.path().join("middle.txt");
    let oldest = temp.path().join("oldest.txt");
    write(&newest, b"hit-new-1\nhit-new-2\n");
    write(&middle, b"hit-mid-1\nhit-mid-2\ntail\n");
    write(&oldest, b"hit-old\n");
    set_mtime(&newest, 1_700_000_003);
    set_mtime(&middle, 1_700_000_002);
    set_mtime(&oldest, 1_700_000_001);

    let mut files = request(temp.path(), "hit", OutputMode::FilesWithMatches);
    files.offset = Some(1);
    files.head_limit = Some(1);
    assert_eq!(
        text(grep_files(files)),
        format!(
            "{}\n\n(Partial: file 2 shown; more exist. Continue with offset=2.)",
            normalized(&middle)
        )
    );

    let mut count = request(temp.path(), "hit", OutputMode::Count);
    count.offset = Some(1);
    count.head_limit = Some(1);
    assert_eq!(
        text(grep_files(count)),
        format!(
            "{}:2\n\n(Partial: 1 file shown, page subtotal 2 occurrences; more exist. Continue with offset=2.)",
            normalized(&middle)
        )
    );

    let mut content = request(temp.path(), "hit", OutputMode::Content);
    content.offset = Some(3);
    content.head_limit = Some(1);
    content.context = Some(1);
    assert_eq!(
        text(grep_files(content)),
        format!(
            "{}\n1-hit-mid-1\n2:hit-mid-2\n3-tail\n\n(Partial: result 4 shown; more exist. Continue with offset=4.)",
            normalized(&middle)
        )
    );
}

#[test]
fn grep_offset_pages_reassemble_the_unlimited_result() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("pages.txt");
    write(&file, b"hit-1\nhit-2\nhit-3\nhit-4\nhit-5");

    let expected = "1:hit-1\n2:hit-2\n3:hit-3\n4:hit-4\n5:hit-5";

    let mut combined = Vec::new();
    for offset in [0, 2, 4] {
        let mut page = request(&file, "hit", OutputMode::Content);
        page.head_limit = Some(2);
        page.offset = Some(offset);
        let output = text(grep_files(page));
        combined.extend(
            output
                .lines()
                .filter(|line| !line.is_empty() && !line.starts_with('('))
                .map(str::to_string),
        );
    }
    assert_eq!(combined.join("\n"), expected);
}

#[test]
fn grep_summary_is_global_while_count_reports_only_the_page_subtotal() {
    let temp = tempfile::tempdir().unwrap();
    let newest = temp.path().join("newest.txt");
    let oldest = temp.path().join("oldest.txt");
    write(&newest, b"hit hit\n");
    write(&oldest, b"hit\n");
    set_mtime(&newest, 1_700_000_002);
    set_mtime(&oldest, 1_700_000_001);

    let mut count = request(temp.path(), "hit", OutputMode::Count);
    count.head_limit = Some(1);
    assert_eq!(
        text(grep_files(count)),
        format!(
            "{}:2\n\n(Partial: 1 file shown, page subtotal 2 occurrences; more exist. Continue with offset=1.)",
            normalized(&newest)
        )
    );

    let mut summary = request(temp.path(), "hit", OutputMode::Summary);
    summary.head_limit = Some(1);
    summary.offset = Some(999);
    assert_eq!(
        text(grep_files(summary)),
        "(Complete: 3 occurrences across 2 files.)"
    );

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "hit hit",
            OutputMode::Count
        ))),
        format!(
            "{}:1\n\n(Complete: 1 occurrence across 1 file.)",
            normalized(&newest)
        )
    );
    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "hit hit",
            OutputMode::Summary
        ))),
        "(Complete: 1 occurrence across 1 file.)"
    );

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "absent",
            OutputMode::Summary
        ))),
        "(Complete: 0 occurrences across 0 files.)"
    );
}

#[test]
fn grep_offset_exhaustion_never_lies_about_a_nonempty_result_set() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("matches.txt");
    write(&file, b"hit one\nhit two\n");

    let mut content = request(&file, "hit", OutputMode::Content);
    content.offset = Some(2);
    assert_eq!(
        text(grep_files(content)),
        "(Complete: no results at offset=2; only 2 results exist.)"
    );

    for mode in [OutputMode::FilesWithMatches, OutputMode::Count] {
        let mut input = request(&file, "hit", mode);
        input.offset = Some(1);
        assert_eq!(
            text(grep_files(input)),
            "(Complete: no files at offset=1; only 1 file exists.)"
        );
    }

    let mut empty = request(&file, "absent", OutputMode::Content);
    empty.offset = Some(50);
    assert_eq!(text(grep_files(empty)), "(Complete: no matches found.)");
}

#[test]
fn grep_long_lines_keep_a_match_window_and_omit_long_context() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("long.txt");
    write(
        &file,
        format!(
            "{}\n{}NEEDLE{}\n",
            "c".repeat(600),
            "a".repeat(300),
            "b".repeat(300)
        ),
    );
    let mut input = request(&file, "NEEDLE", OutputMode::Content);
    input.before_context = Some(1);
    let output = text(grep_files(input));
    assert!(output.starts_with("1-[long line omitted: 600 chars]\n2:…"));
    assert!(output.contains("NEEDLE"));
    assert!(output.contains("[line is 606 chars; showing window around match(es)]"));
}

#[test]
fn grep_long_line_threshold_is_exactly_five_hundred_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("threshold.txt");
    let at_limit = format!("{}NEEDLE{}", "a".repeat(247), "b".repeat(247));
    let over_limit = format!("{}NEEDLE{}", "c".repeat(247), "d".repeat(248));
    assert_eq!(at_limit.len(), 500);
    assert_eq!(over_limit.len(), 501);
    write(&file, format!("{at_limit}\n{over_limit}"));
    let output = text(grep_files(request(&file, "NEEDLE", OutputMode::Content)));
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], format!("1:{at_limit}"));
    assert!(lines[1].starts_with("2:…"));
    assert!(lines[1].ends_with("[line is 501 chars; showing window around match(es)]"));
}

#[test]
fn grep_long_line_ellipses_only_mark_sides_that_were_omitted() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sides.txt");
    write(
        &file,
        format!("NEEDLE{}\n{}NEEDLE", "a".repeat(600), "b".repeat(600)),
    );
    let output = text(grep_files(request(&file, "NEEDLE", OutputMode::Content)));
    let lines = output.lines().collect::<Vec<_>>();
    assert!(lines[0].starts_with("1:NEEDLE"));
    assert!(!lines[0].starts_with("1:…"));
    assert!(lines[0].contains("… [line is"));
    assert!(lines[1].starts_with("2:…"));
    assert!(!lines[1].contains("NEEDLE… [line is"));
}

#[test]
fn grep_long_line_window_covers_nearby_matches_and_discloses_distant_ones() {
    let temp = tempfile::tempdir().unwrap();
    let nearby = temp.path().join("nearby.txt");
    write(
        &nearby,
        format!(
            "{}FIRST{}SECOND{}",
            "a".repeat(300),
            "b".repeat(50),
            "c".repeat(300)
        ),
    );
    let nearby_output = text(grep_files(request(
        &nearby,
        "FIRST|SECOND",
        OutputMode::Content,
    )));
    assert_eq!(nearby_output.matches("FIRST").count(), 1);
    assert_eq!(nearby_output.matches("SECOND").count(), 1);
    assert!(nearby_output.contains("showing window around match(es)"));
    assert!(!nearby_output.contains("additional matches fall outside this window"));

    let distant = temp.path().join("distant.txt");
    write(
        &distant,
        format!(
            "{}FIRST{}SECOND{}",
            "a".repeat(300),
            "b".repeat(2_200),
            "c".repeat(300)
        ),
    );
    let distant_output = text(grep_files(request(
        &distant,
        "FIRST|SECOND",
        OutputMode::Content,
    )));
    assert!(distant_output.contains("FIRST"));
    assert!(!distant_output.contains("SECOND"));
    assert!(distant_output.contains("additional matches fall outside this window"));
}

#[test]
fn grep_truncates_an_oversized_match_body_with_an_explicit_marker() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("huge-match.txt");
    write(&file, "x".repeat(2_100));
    let mut input = request(&file, "x+", OutputMode::Content);
    input.only_matching = Some(true);
    let output = text(grep_files(input));
    assert!(output.starts_with(&format!("1:{}", "x".repeat(2_000))));
    assert!(output.contains("... [match truncated: 2100 chars total]"));
    assert!(output.ends_with("(Complete: all 1 result shown.)"));
}

#[test]
fn grep_rejects_invalid_regex_and_type_with_exact_guidance() {
    let temp = tempfile::tempdir().unwrap();
    let regex_error = error_text(grep_files(request(
        temp.path(),
        "(?=x)",
        OutputMode::Content,
    )));
    assert!(regex_error.starts_with("Invalid regex pattern:"));
    assert!(regex_error.ends_with(
        "Note: Rust regex syntax — no lookaround or backreferences; escape literal braces."
    ));

    let mut type_error = request(temp.path(), "x", OutputMode::Content);
    type_error.file_type = Some("definitely-not-a-type".to_string());
    assert_eq!(
        error_text(grep_files(type_error)),
        "Unknown file type: \"definitely-not-a-type\". Run with a glob filter instead, or use a standard type like js, py, rust, go, java."
    );
}

#[test]
fn grep_multi_file_content_and_empty_modes_have_exact_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let older = temp.path().join("older.txt");
    let newer = temp.path().join("newer.txt");
    write(&older, b"before\nhit old\nafter\nfar\n");
    write(&newer, b"start\nhit new\nend\n");
    set_mtime(&older, 1_700_000_001);
    set_mtime(&newer, 1_700_000_002);
    let mut input = request(temp.path(), "hit", OutputMode::Content);
    input.context = Some(1);
    assert_eq!(
        text(grep_files(input)),
        format!(
            "{}\n1-start\n2:hit new\n3-end\n\n{}\n1-before\n2:hit old\n3-after\n\n(Complete: all 2 results shown.)",
            normalized(&newer),
            normalized(&older),
        )
    );

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "absent",
            OutputMode::Content
        ))),
        "(Complete: no matches found.)"
    );
    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "absent",
            OutputMode::FilesWithMatches
        ))),
        "(Complete: no files matched.)"
    );
    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "absent",
            OutputMode::Count
        ))),
        "(Complete: no matches found.)"
    );
}

#[test]
fn grep_filters_case_line_numbers_and_context_precedence_are_observable() {
    let temp = tempfile::tempdir().unwrap();
    let rust = temp.path().join("src/main.rs");
    let text_file = temp.path().join("src/main.txt");
    write(&rust, b"zero\nNeedle\ntwo\nthree\n");
    write(&text_file, b"Needle\n");

    let mut filtered = request(temp.path(), "needle", OutputMode::Content);
    filtered.case_insensitive = Some(true);
    filtered.file_type = Some("rust".to_string());
    filtered.glob = Some("**/*.rs".to_string());
    filtered.line_numbers = Some(false);
    filtered.before_context = Some(0);
    filtered.after_context = Some(0);
    filtered.context = Some(1);
    assert_eq!(
        text(grep_files(filtered)),
        format!(
            "{}\nzero\nNeedle\ntwo\n\n(Complete: all 1 result shown.)",
            normalized(&rust)
        )
    );
}

#[test]
fn grep_without_line_numbers_keeps_file_and_block_structure() {
    let temp = tempfile::tempdir().unwrap();
    let newer = temp.path().join("newer.txt");
    let older = temp.path().join("older.txt");
    write(&newer, b"hit-new-1\ngap\nhit-new-2\n");
    write(&older, b"hit-old\n");
    set_mtime(&newer, 1_700_000_002);
    set_mtime(&older, 1_700_000_001);

    let mut input = request(temp.path(), "hit", OutputMode::Content);
    input.line_numbers = Some(false);
    assert_eq!(
        text(grep_files(input)),
        format!(
            "{}\nhit-new-1\n--\nhit-new-2\n\n{}\nhit-old\n\n(Complete: all 3 results shown.)",
            normalized(&newer),
            normalized(&older)
        )
    );
}

#[test]
fn grep_skips_binary_and_orders_multiple_encoding_notes() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("binary.dat");
    write(&binary, b"hit\0hidden");

    let utf16 = temp.path().join("utf16.txt");
    let mut utf16_bytes = vec![0xFF, 0xFE];
    for unit in "hit utf16".encode_utf16() {
        utf16_bytes.extend(unit.to_le_bytes());
    }
    write(&utf16, utf16_bytes);
    let gbk = temp.path().join("gbk.txt");
    let gbk_source = "hit 中文编码验证文本足够长，确保自动检测拥有充分证据";
    write(
        &gbk,
        hex::decode("68697420d6d0cec4b1e0c2ebd1e9d6a4cec4b1bed7e3b9bbb3a4a3acc8b7b1a3d7d4b6afbcecb2e2d3b5d3d0b3e4b7d6d6a4bedd")
            .unwrap(),
    );
    set_mtime(&utf16, 1_700_000_002);
    set_mtime(&gbk, 1_700_000_001);

    assert_eq!(
        text(grep_files(request(temp.path(), "hit", OutputMode::Content))),
        format!(
            "{}\n1:hit utf16\n\n{}\n1:{gbk_source}\n\n(Note: decoded from GBK; output is UTF-8.)\n(Note: decoded from UTF-16LE; output is UTF-8.)\n(Complete: all 2 results shown.)",
            normalized(&utf16),
            normalized(&gbk)
        )
    );
}

#[test]
fn grep_reports_undetermined_encodings_and_errors_for_single_file_targets() {
    let temp = tempfile::tempdir().unwrap();
    let valid = temp.path().join("valid.txt");
    let ambiguous = temp.path().join("ambiguous.txt");
    let undecodable = temp.path().join("undecodable.txt");
    write(&valid, b"hit\n");
    write(&ambiguous, b"valid\xFFtail");
    write(&undecodable, [0x81]);
    set_mtime(&ambiguous, 1_700_000_003);
    set_mtime(&undecodable, 1_700_000_002);
    set_mtime(&valid, 1_700_000_001);

    assert_eq!(
        text(grep_files(request(temp.path(), "hit", OutputMode::Content))),
        format!(
            "{}\n1:hit\n\n{} — ambiguous: windows-1252\n{} — undecodable\n(Complete: all 1 result shown; 2 files skipped.)",
            normalized(&valid),
            normalized(&ambiguous),
            normalized(&undecodable)
        )
    );

    assert_eq!(
        error_text(grep_files(request(&ambiguous, "hit", OutputMode::Content))),
        format!(
            "Cannot determine the text encoding of {} with confidence: the bytes decode cleanly as windows-1252. Retry with encoding=\"...\" if the context tells you which one, or use view=\"hex\".",
            normalized(&ambiguous)
        )
    );
    assert_eq!(
        error_text(grep_files(request(
            &undecodable,
            "hit",
            OutputMode::Content
        ))),
        format!(
            "Cannot decode {} as text: no supported encoding decodes it cleanly. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&undecodable)
        )
    );

    let single_dir = temp.path().join("single-skip");
    let single_valid = single_dir.join("valid.txt");
    let single_ambiguous = single_dir.join("ambiguous.txt");
    write(&single_valid, b"hit");
    write(&single_ambiguous, b"valid\xFFtail");
    set_mtime(&single_ambiguous, 1_700_000_010);
    set_mtime(&single_valid, 1_700_000_009);
    assert_eq!(
        text(grep_files(request(
            &single_dir,
            "hit",
            OutputMode::FilesWithMatches
        ))),
        format!(
            "{}\n\n{} — ambiguous: windows-1252\n(Complete: all 1 file shown; 1 file skipped.)",
            normalized(&single_valid),
            normalized(&single_ambiguous)
        )
    );
}

#[test]
fn grep_encoding_skip_report_is_inline_for_summary_and_partial_pages() {
    let temp = tempfile::tempdir().unwrap();
    let mut ambiguous_paths = Vec::new();
    for index in 0..6 {
        let path = temp.path().join(format!("ambiguous-{index}.txt"));
        write(&path, b"valid\xFFtail");
        set_mtime(&path, 1_700_000_100 - index as i64);
        ambiguous_paths.push(path);
    }
    let matches = temp.path().join("matches.txt");
    write(&matches, b"hit one\nhit two\n");
    set_mtime(&matches, 1_700_000_001);

    let summary = text(grep_files(request(temp.path(), "hit", OutputMode::Summary)));
    assert_eq!(
        summary,
        format!(
            "{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n(Complete: 2 occurrences across 1 file; 6 files skipped.)",
            normalized(&ambiguous_paths[0]),
            normalized(&ambiguous_paths[1]),
            normalized(&ambiguous_paths[2]),
            normalized(&ambiguous_paths[3]),
            normalized(&ambiguous_paths[4]),
            normalized(&ambiguous_paths[5])
        )
    );

    let mut first_page = request(temp.path(), "hit", OutputMode::Content);
    first_page.head_limit = Some(1);
    assert_eq!(
        text(grep_files(first_page)),
        format!(
            "{}\n1:hit one\n\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n{} — ambiguous: windows-1252\n(Partial: result 1 shown; more exist. Continue with offset=1; 6 files skipped.)",
            normalized(&matches),
            normalized(&ambiguous_paths[0]),
            normalized(&ambiguous_paths[1]),
            normalized(&ambiguous_paths[2]),
            normalized(&ambiguous_paths[3]),
            normalized(&ambiguous_paths[4]),
            normalized(&ambiguous_paths[5])
        )
    );
}

#[test]
fn grep_searches_utf32_bom_text_and_keeps_binary_files_silent() {
    let temp = tempfile::tempdir().unwrap();
    let utf32 = temp.path().join("utf32.txt");
    let mut bytes = vec![0xFF, 0xFE, 0x00, 0x00];
    for character in "before\nhit UTF32".chars() {
        bytes.extend((character as u32).to_le_bytes());
    }
    write(&utf32, bytes);
    let binary = temp.path().join("archive.zip");
    write(&binary, b"PK\x03\x04\0hit");
    set_mtime(&binary, 1_700_000_002);
    set_mtime(&utf32, 1_700_000_001);

    assert_eq!(
        text(grep_files(request(temp.path(), "hit", OutputMode::Content))),
        format!(
            "{}\n2:hit UTF32\n\n(Note: decoded from UTF-32LE; output is UTF-8.)\n(Complete: all 1 result shown.)",
            normalized(&utf32)
        )
    );
}

#[test]
fn grep_single_file_encoding_is_explicit_and_scope_misuse_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("short-gbk.txt");
    write(&file, hex::decode("d6d0cec4").unwrap());

    let mut explicit = request(&file, "中文", OutputMode::Content);
    explicit.encoding = Some("gbk".to_string());
    assert_eq!(
        text(grep_files(explicit)),
        "1:中文\n\n(Note: decoded from GBK as requested; output is UTF-8.)\n(Complete: all 1 result shown.)"
    );

    let mut directory_encoding = request(temp.path(), "中文", OutputMode::Content);
    directory_encoding.encoding = Some("gbk".to_string());
    assert_eq!(
        error_text(grep_files(directory_encoding)),
        "The encoding parameter only applies to single-file targets; use fallback_encoding for a directory."
    );

    let mut file_fallback = request(&file, "中文", OutputMode::Content);
    file_fallback.fallback_encoding = Some("gbk".to_string());
    assert_eq!(
        error_text(grep_files(file_fallback)),
        "The fallback_encoding parameter only applies to directory targets; use encoding for a single file."
    );

    let valid_utf8 = temp.path().join("valid-utf8.txt");
    write(&valid_utf8, "中文".repeat(12));
    let mut suspicious = request(&valid_utf8, "definitely-absent", OutputMode::Content);
    suspicious.encoding = Some("gbk".to_string());
    assert_eq!(
        text(grep_files(suspicious)),
        "(Note: decoded from GBK as requested; output is UTF-8. Warning: the raw bytes are also valid UTF-8 — if this looks garbled, retry with encoding=\"utf-8\" or omit encoding.)\n(Complete: no matches found.)"
    );
}

#[test]
fn grep_directory_fallback_only_decodes_unresolved_files_and_reports_residual_skips() {
    let temp = tempfile::tempdir().unwrap();
    let utf8 = temp.path().join("utf8.txt");
    let utf16 = temp.path().join("utf16.txt");
    let fallback = temp.path().join("fallback-gbk.txt");
    let undecodable = temp.path().join("undecodable.txt");
    write(&utf8, b"hit utf8");
    let mut utf16_bytes = vec![0xFF, 0xFE];
    for unit in "hit utf16".encode_utf16() {
        utf16_bytes.extend(unit.to_le_bytes());
    }
    write(&utf16, utf16_bytes);
    let mut fallback_bytes = b"hit ".to_vec();
    fallback_bytes.extend(hex::decode("d6d0cec4").unwrap());
    write(&fallback, fallback_bytes);
    write(&undecodable, [0x81]);
    set_mtime(&utf8, 1_700_000_004);
    set_mtime(&utf16, 1_700_000_003);
    set_mtime(&fallback, 1_700_000_002);
    set_mtime(&undecodable, 1_700_000_001);

    let mut input = request(temp.path(), "hit", OutputMode::Content);
    input.fallback_encoding = Some("gbk".to_string());
    assert_eq!(
        text(grep_files(input)),
        format!(
            "{}\n1:hit utf8\n\n{}\n1:hit utf16\n\n{}\n1:hit 中文\n\n(Note: decoded from UTF-16LE; output is UTF-8.)\n{} — undecodable\n(Note: 1 file decoded using fallback encoding GBK.)\n(Complete: all 3 results shown; 1 file skipped.)",
            normalized(&utf8),
            normalized(&utf16),
            normalized(&fallback),
            normalized(&undecodable)
        )
    );
}

#[test]
fn grep_directory_fallback_never_overrides_a_bom_content_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("broken-utf8-bom.txt");
    write(&path, [0xEF, 0xBB, 0xBF, 0x81]);

    let mut input = request(temp.path(), "hit", OutputMode::Content);
    input.fallback_encoding = Some("gbk".to_string());
    assert_eq!(
        text(grep_files(input)),
        format!(
            "{} — undecodable\n(Complete: no matches found; 1 file skipped.)",
            normalized(&path)
        )
    );
}

#[test]
fn grep_mixed_and_iso_2022_files_never_turn_into_false_no_match_claims() {
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("mixed.txt");
    let iso = temp.path().join("iso-2022.txt");
    let mut mixed_bytes = "needle UTF-8 前缀内容足够清晰，包含多字节字符。\n"
        .as_bytes()
        .to_vec();
    mixed_bytes.extend(
        hex::decode("d6d0cec4cbd1cbf7b1e0c2ebd1e9d6a4cec4b1bed7e3b9bbb3a40ab5dab6fed0d0bcccd0f8b0fcbaacb8fcb6e0d6d0cec4d7d6b7fb")
            .unwrap(),
    );
    write(&mixed, mixed_bytes);
    write(&iso, b"needle \x1B$B$3$s$K$A$O\x1B(B");
    set_mtime(&mixed, 1_700_000_002);
    set_mtime(&iso, 1_700_000_001);

    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "needle",
            OutputMode::Content
        ))),
        format!(
            "{} — mixed or inconsistent encodings\n{} — ambiguous: iso-2022-jp\n(Complete: no matches found; 2 files skipped.)",
            normalized(&mixed),
            normalized(&iso)
        )
    );
}

#[test]
fn grep_invalid_encoding_labels_fail_even_when_the_directory_is_empty() {
    let temp = tempfile::tempdir().unwrap();
    let mut input = request(temp.path(), "hit", OutputMode::Content);
    input.fallback_encoding = Some("not-an-encoding".to_string());
    assert_eq!(
        error_text(grep_files(input)),
        "Invalid encoding value \"not-an-encoding\". Use a WHATWG encoding label such as \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", \"windows-1252\", \"utf-16le\", or \"utf-32le\"."
    );
}

#[test]
fn grep_missing_path_and_invalid_glob_fail_with_a_next_step() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        error_text(grep_files(request(&missing, "hit", OutputMode::Content))),
        format!(
            "Path does not exist: {}\nNote: the session working directory is {}.",
            normalized(&missing),
            cwd()
        )
    );

    let mut invalid_glob = request(temp.path(), "hit", OutputMode::Content);
    invalid_glob.glob = Some("[".to_string());
    let error = error_text(grep_files(invalid_glob));
    assert!(error.starts_with("Invalid glob pattern:"));
    assert!(error.ends_with("Use forms like \"*.rs\" or \"**/*.{ts,tsx}\"."));
}

#[test]
fn grep_rejects_a_single_line_beyond_the_search_buffer_limit() {
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("oversized-line.txt");
    let mut handle = std::fs::File::create(&file).unwrap();
    handle.write_all(&vec![b'a'; 8 * 1024]).unwrap();
    handle.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(handle);
    assert_eq!(
        error_text(grep_files(request(&file, "a", OutputMode::Content))),
        format!(
            "Cannot search file {}: a line or multiline buffer exceeds the 64 MiB safety limit. Narrow the path or search without multiline.",
            normalized(&file)
        )
    );
}

#[test]
fn grep_rejects_captured_content_beyond_the_capture_limit() {
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("oversized-capture.txt");
    let mut handle = std::fs::File::create(&file).unwrap();
    handle.write_all(&vec![b'a'; 8 * 1024]).unwrap();
    handle.set_len(33 * 1024 * 1024).unwrap();
    drop(handle);
    let mut input = request(&file, ".*", OutputMode::Content);
    input.only_matching = Some(true);
    assert_eq!(
        error_text(grep_files(input)),
        format!(
            "Cannot search file {}: matching content and context exceed the 64 MiB safety limit. Narrow the pattern or reduce context.",
            normalized(&file)
        )
    );
}

#[test]
fn grep_relative_existing_path_gives_the_absolute_path_to_retry() {
    let input = GrepRequest {
        pattern: "readme".to_string(),
        path: Some("README.md".to_string()),
        glob: None,
        file_type: None,
        output_mode: Some(OutputMode::Content),
        case_insensitive: None,
        line_numbers: None,
        only_matching: None,
        before_context: None,
        after_context: None,
        context: None,
        multiline: None,
        head_limit: None,
        offset: None,
        encoding: None,
        fallback_encoding: None,
    };
    assert_eq!(
        error_text(grep_files(input)),
        format!(
            "Path does not exist: README.md\nNote: the session working directory is {}. Use the absolute path {}/README.md.",
            cwd(),
            cwd()
        )
    );
}

#[cfg(unix)]
#[test]
fn grep_limit_probe_does_not_open_later_unneeded_files() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.txt");
    let later = temp.path().join("later.txt");
    write(&first, "hit\n".repeat(251));
    write(&later, b"hit");
    set_mtime(&first, 1_700_000_002);
    set_mtime(&later, 1_700_000_001);
    let original = fs::metadata(&later).unwrap().permissions();
    fs::set_permissions(&later, fs::Permissions::from_mode(0o0)).unwrap();
    let response = grep_files(request(temp.path(), "hit", OutputMode::Content));
    fs::set_permissions(&later, original).unwrap();
    assert!(!response.is_error, "{response:?}");
}

#[cfg(windows)]
#[test]
fn grep_limit_probe_does_not_open_later_unneeded_files() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.txt");
    let later = temp.path().join("later.txt");
    write(&first, "hit\n".repeat(251));
    write(&later, b"hit");
    set_mtime(&first, 1_700_000_002);
    set_mtime(&later, 1_700_000_001);
    let _lock = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&later)
        .unwrap();
    let response = grep_files(request(temp.path(), "hit", OutputMode::Content));
    assert!(!response.is_error, "{response:?}");
}

#[cfg(unix)]
#[test]
fn grep_lists_non_utf8_filenames_lossily_without_dropping_legal_neighbors() {
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().unwrap();
    let invalid = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"bad-\xFF.txt".to_vec()));
    let legal = temp.path().join("legal.txt");
    // APFS rejects file names that are not valid UTF-8; the fixture is only
    // creatable on filesystems that accept arbitrary bytes, e.g. ext4 (2026-07-16).
    if std::fs::write(&invalid, b"hit").is_err() {
        eprintln!("skipping: this filesystem rejects non-UTF-8 file names");
        return;
    }
    write(&legal, b"hit");
    set_mtime(&invalid, 1_700_000_002);
    set_mtime(&legal, 1_700_000_001);

    let invalid_display = normalized(&invalid);
    assert!(invalid_display.contains('\u{FFFD}'));
    assert_eq!(
        text(grep_files(request(
            temp.path(),
            "hit",
            OutputMode::FilesWithMatches
        ))),
        format!(
            "{}\n{}\n\n(Complete: all 2 files shown.)",
            invalid_display,
            normalized(&legal)
        )
    );
}

fn fs_create_dir(path: std::path::PathBuf) {
    std::fs::create_dir_all(path).unwrap();
}
