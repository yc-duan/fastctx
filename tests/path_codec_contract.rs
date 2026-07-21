mod common;

use common::{error_text, glob_files, grep_files, normalized, text, write};
use fastctx::glob_tool::{FilterMode, GlobRequest};
use fastctx::grep_tool::{GrepRequest, OutputMode};

fn glob_request(root: &std::path::Path, pattern: &str) -> GlobRequest {
    GlobRequest {
        pattern: pattern.to_string(),
        path: Some(normalized(root)),
        filter_mode: Some(FilterMode::All),
        sort: None,
        offset: None,
        limit: None,
    }
}

fn grep_request(path: String) -> GrepRequest {
    GrepRequest {
        pattern: "needle".to_string(),
        path: Some(path),
        glob: None,
        file_type: None,
        output_mode: Some(OutputMode::FilesWithMatches),
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
fn portable_line_separators_have_fixed_tokens_and_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("line\u{2028}break.txt");
    let second = temp.path().join("line\u{2029}break.txt");
    write(&first, b"needle");
    write(&second, b"needle");

    let root = normalized(temp.path());
    let first_display = format!("{root}/~fastctx~b6c696e65e280a8627265616b2e747874~");
    let second_display = format!("{root}/~fastctx~b6c696e65e280a9627265616b2e747874~");
    assert_eq!(
        text(glob_files(glob_request(temp.path(), "**/*"))),
        format!("{first_display}\n{second_display}\n\n(Complete: all 2 files shown.)")
    );
    assert_eq!(
        text(grep_files(grep_request(first_display.clone()))),
        format!("{first_display}\n\n(Complete: all 1 file shown.)")
    );
}

#[test]
fn literal_canonical_token_is_outer_encoded_and_decoded_once() {
    let temp = tempfile::tempdir().unwrap();
    let canonical_literal = temp.path().join("~fastctx~b61~");
    let malformed_literal = temp.path().join("~fastctx~bzz~");
    write(&canonical_literal, b"needle");
    write(&malformed_literal, b"needle");

    let root = normalized(temp.path());
    let outer = format!("{root}/~fastctx~b7e666173746374787e6236317e~");
    let malformed = format!("{root}/~fastctx~bzz~");
    assert_eq!(
        text(glob_files(glob_request(temp.path(), "**/*"))),
        format!("{outer}\n{malformed}\n\n(Complete: all 2 files shown.)")
    );

    let exact = glob_files(glob_request(
        temp.path(),
        "~fastctx~b7e666173746374787e6236317e~",
    ));
    assert_eq!(
        text(exact),
        format!("{outer}\n\n(Complete: all 1 file shown.)")
    );
    assert_eq!(
        text(grep_files(grep_request(outer.clone()))),
        format!("{outer}\n\n(Complete: all 1 file shown.)")
    );
}

#[test]
fn complete_b_token_with_a_separator_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let token = "~fastctx~b2f~";
    let mut input = glob_request(temp.path(), "*");
    input.path = Some(format!("{}/{token}", normalized(temp.path())));
    assert_eq!(
        error_text(glob_files(input)),
        format!(
            "Invalid FastCtx-encoded path component \"{token}\" for this platform. Copy an encoded path exactly as returned by FastCtx."
        )
    );
}

#[cfg(unix)]
#[test]
fn unix_invalid_bytes_do_not_collide_with_a_real_replacement_character() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().unwrap();
    let invalid = temp
        .path()
        .join(OsString::from_vec(b"bad-\xff.txt".to_vec()));
    let replacement = temp.path().join("bad-\u{fffd}.txt");
    if std::fs::write(&invalid, b"needle").is_err() {
        eprintln!("skipping: this filesystem rejects non-UTF-8 file names");
        return;
    }
    write(&replacement, b"needle");

    let root = normalized(temp.path());
    let invalid_display = format!("{root}/~fastctx~b6261642dff2e747874~");
    let replacement_display = format!("{root}/bad-\u{fffd}.txt");
    assert_eq!(
        text(glob_files(glob_request(temp.path(), "*"))),
        format!("{replacement_display}\n{invalid_display}\n\n(Complete: all 2 files shown.)")
    );
    assert_eq!(
        text(glob_files(glob_request(
            temp.path(),
            "~fastctx~b6261642dff2e747874~",
        ))),
        format!("{invalid_display}\n\n(Complete: all 1 file shown.)")
    );
    assert_eq!(
        text(grep_files(grep_request(invalid_display.clone()))),
        format!("{invalid_display}\n\n(Complete: all 1 file shown.)")
    );
}

#[cfg(unix)]
#[test]
fn unix_literal_backslash_and_directory_separator_remain_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let backslash = temp.path().join("a\\b.txt");
    let nested = temp.path().join("a/b.txt");
    write(&backslash, b"needle");
    write(&nested, b"needle");

    let root = normalized(temp.path());
    let backslash_display = format!("{root}/~fastctx~b615c622e747874~");
    let nested_display = format!("{root}/a/b.txt");
    assert_eq!(
        text(glob_files(glob_request(temp.path(), "**/*"))),
        format!("{nested_display}\n{backslash_display}\n\n(Complete: all 2 files shown.)")
    );
    assert_eq!(
        text(glob_files(glob_request(
            temp.path(),
            "~fastctx~b615c622e747874~",
        ))),
        format!("{backslash_display}\n\n(Complete: all 1 file shown.)")
    );
}

#[cfg(unix)]
#[test]
fn unix_control_names_cannot_inject_protocol_lines() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("line\nbreak.txt"), b"needle");
    write(&temp.path().join("tab\tname.txt"), b"needle");
    write(&temp.path().join("esc\u{1b}name.txt"), b"needle");

    let root = normalized(temp.path());
    let esc = format!("{root}/~fastctx~b6573631b6e616d652e747874~");
    let line = format!("{root}/~fastctx~b6c696e650a627265616b2e747874~");
    let tab = format!("{root}/~fastctx~b746162096e616d652e747874~");
    let output = text(glob_files(glob_request(temp.path(), "**/*")));
    assert_eq!(
        output,
        format!("{esc}\n{line}\n{tab}\n\n(Complete: all 3 files shown.)")
    );
    assert_eq!(output.lines().count(), 5);
    assert!(!output.contains('\t'));
    assert!(!output.contains('\u{1b}'));
}

#[cfg(unix)]
#[test]
fn canonical_windows_token_is_reserved_on_unix() {
    let temp = tempfile::tempdir().unwrap();
    let token = "~fastctx~w0061~";
    let mut input = glob_request(temp.path(), "*");
    input.path = Some(format!("{}/{token}", normalized(temp.path())));
    assert_eq!(
        error_text(glob_files(input)),
        format!(
            "Invalid FastCtx-encoded path component \"{token}\" for this platform. Copy an encoded path exactly as returned by FastCtx."
        )
    );
}

#[cfg(windows)]
#[test]
fn non_utf8_b_token_is_reserved_on_windows() {
    let temp = tempfile::tempdir().unwrap();
    let token = "~fastctx~bff~";
    let mut input = glob_request(temp.path(), "*");
    input.path = Some(format!("{}/{token}", normalized(temp.path())));
    assert_eq!(
        error_text(glob_files(input)),
        format!(
            "Invalid FastCtx-encoded path component \"{token}\" for this platform. Copy an encoded path exactly as returned by FastCtx."
        )
    );
}
