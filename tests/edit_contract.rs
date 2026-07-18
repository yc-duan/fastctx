mod common;

use common::{McpSession, mcp_text, normalized, write};
use encoding_rs::{GBK, SHIFT_JIS};
use std::path::Path;
use std::process::Command;

#[test]
fn replace_is_default_and_deprecated_enable_edit_is_a_noop() {
    let temp = tempfile::tempdir().unwrap();
    let mut default = edit_session(temp.path(), None);
    assert_eq!(default.list_tools(), ["glob", "grep", "read", "replace"]);
    assert!(default.close().success());

    let mut compatibility = McpSession::start(edit_command(temp.path(), None, true));
    assert_eq!(
        compatibility.list_tools(),
        ["glob", "grep", "read", "replace"]
    );
    assert!(compatibility.close().success());
}

#[test]
fn replace_preserves_raw_bytes_across_eol_and_legacy_encodings() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = edit_session(temp.path(), None);

    let mixed = temp.path().join("mixed.txt");
    write(&mixed, b"prefix\r\nfoo\nunchanged\r\n");
    replace_file(&mut session, &mixed, "foo", "bar", None);
    assert_eq!(
        std::fs::read(&mixed).unwrap(),
        b"prefix\r\nbar\nunchanged\r\n"
    );

    let gbk_path = temp.path().join("gbk.txt");
    let gbk_text = format!("{}旧值{}\r\n", "中文前缀".repeat(10), "中文后缀".repeat(10));
    write(&gbk_path, encode_legacy(GBK, &gbk_text));
    replace_file(&mut session, &gbk_path, "旧值", "新值", Some("gbk"));
    assert_eq!(
        std::fs::read(&gbk_path).unwrap(),
        encode_legacy(GBK, &gbk_text.replace("旧值", "新值"))
    );

    let sjis_path = temp.path().join("sjis.txt");
    let sjis_text = "前方そのまま\r\n古い値\r\n後方そのまま";
    write(&sjis_path, encode_legacy(SHIFT_JIS, sjis_text));
    replace_file(
        &mut session,
        &sjis_path,
        "古い値",
        "新しい値",
        Some("shift_jis"),
    );
    assert_eq!(
        std::fs::read(&sjis_path).unwrap(),
        encode_legacy(SHIFT_JIS, &sjis_text.replace("古い値", "新しい値"))
    );

    let utf16 = temp.path().join("utf16.txt");
    let mut utf16_raw = vec![0xff, 0xfe];
    for unit in "prefix\r\nold\r\nsuffix".encode_utf16() {
        utf16_raw.extend(unit.to_le_bytes());
    }
    write(&utf16, &utf16_raw);
    replace_file(&mut session, &utf16, "old", "NEW", None);
    let mut utf16_expected = vec![0xff, 0xfe];
    for unit in "prefix\r\nNEW\r\nsuffix".encode_utf16() {
        utf16_expected.extend(unit.to_le_bytes());
    }
    assert_eq!(std::fs::read(&utf16).unwrap(), utf16_expected);
    assert!(session.close().success());
}

#[test]
fn replace_supports_captures_deletion_flags_zero_width_and_blast_radius() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = edit_session(temp.path(), None);

    let captures = temp.path().join("captures.txt");
    write(&captures, b"ab ab");
    replace_file(&mut session, &captures, "(a)(b)", "$2$1$$", None);
    assert_eq!(std::fs::read(&captures).unwrap(), b"ba$ ba$");

    let named = temp.path().join("named.txt");
    write(&named, b"key=42");
    replace_file(
        &mut session,
        &named,
        "(?P<number>[0-9]+)",
        "${number}0",
        None,
    );
    assert_eq!(std::fs::read(&named).unwrap(), b"key=420");

    for (replacement, expected_ref) in [("$2", "$2"), ("${missing}", "${missing}")] {
        let path = temp
            .path()
            .join(format!("unknown-{}.txt", expected_ref.len()));
        write(&path, b"x");
        let response = session.call(
            "replace",
            serde_json::json!({
                "path":normalized(&path), "pattern":"(x)", "replacement":replacement
            }),
        );
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            mcp_text(&response),
            format!(
                "Replacement references an undefined capture group: {expected_ref}. The pattern defines group 1. Fix the replacement; nothing was written."
            )
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"x");
    }

    let deleted = temp.path().join("deleted.txt");
    write(&deleted, b"keep\r\nremove\r\nstay\r\n");
    replace_file(&mut session, &deleted, "remove\n", "", None);
    assert_eq!(std::fs::read(&deleted).unwrap(), b"keep\r\nstay\r\n");

    let flags = temp.path().join("flags.txt");
    write(&flags, b"A.b\na--c");
    let literal = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&flags), "pattern":"a.b", "replacement":"literal",
            "literal":true, "case_insensitive":true
        }),
    );
    assert_eq!(literal["result"]["isError"], false);
    assert_eq!(std::fs::read(&flags).unwrap(), b"literal\na--c");
    let dot_all = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&flags), "pattern":"literal.*c", "replacement":"done",
            "dot_all":true
        }),
    );
    assert_eq!(dot_all["result"]["isError"], false);
    assert_eq!(std::fs::read(&flags).unwrap(), b"done");

    let zero = temp.path().join("zero.txt");
    write(&zero, b"ab");
    let empty_pattern = session.call(
        "replace",
        serde_json::json!({"path":normalized(&zero), "pattern":"", "replacement":"x"}),
    );
    assert_eq!(empty_pattern["result"]["isError"], true);
    assert_eq!(
        mcp_text(&empty_pattern),
        "An empty pattern matches at every position and is almost always a mistake. Give a non-empty pattern."
    );
    let unguarded = session.call(
        "replace",
        serde_json::json!({"path":normalized(&zero), "pattern":"x*", "replacement":"x"}),
    );
    assert_eq!(unguarded["result"]["isError"], true);
    assert_eq!(
        mcp_text(&unguarded),
        "This pattern can match empty (zero-width) and would insert at every position. Set max_replacements to cap the blast radius, then retry."
    );
    let guarded_zero = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&zero), "pattern":"x*", "replacement":"x",
            "max_replacements":3
        }),
    );
    assert_eq!(guarded_zero["result"]["isError"], false);
    assert_eq!(std::fs::read(&zero).unwrap(), b"xaxbx");

    let guarded = temp.path().join("guarded.txt");
    write(&guarded, b"hit hit hit");
    let before = std::fs::read(&guarded).unwrap();
    let response = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&guarded), "pattern":"hit", "replacement":"miss",
            "max_replacements":2
        }),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        mcp_text(&response),
        "Refusing to write: 3 matches exceed max_replacements=2. Raise the cap or narrow the pattern; nothing was written."
    );
    assert_eq!(std::fs::read(&guarded).unwrap(), before);

    let dry = temp.path().join("dry.txt");
    write(&dry, b"one hit\ntwo hit\n");
    let before = std::fs::read(&dry).unwrap();
    let response = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&dry), "pattern":"hit", "replacement":"MISS", "dry_run":true
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    assert!(mcp_text(&response).contains("1: hit -> MISS"));
    assert!(mcp_text(&response).contains("2: hit -> MISS"));
    assert!(
        mcp_text(&response)
            .ends_with("(Complete: dry run — 2 matches in 1 file; nothing written.)")
    );
    assert_eq!(std::fs::read(&dry).unwrap(), before);
    assert!(session.close().success());
}

#[test]
fn directory_replace_lists_binary_and_encoding_skips_without_losing_totals() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("tree");
    std::fs::create_dir_all(&root).unwrap();
    write(&root.join("text.txt"), b"hit\n");
    write(&root.join("binary.dat"), b"hit\0binary");
    write(&root.join("ambiguous.txt"), b"valid\xfftail");
    let mut session = edit_session(temp.path(), None);
    let response = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&root), "pattern":"hit", "replacement":"done"
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    let text = mcp_text(&response);
    assert!(text.contains("binary.dat — skipped: binary file"), "{text}");
    assert!(
        text.contains("ambiguous.txt — skipped: ambiguous encoding"),
        "{text}"
    );
    assert!(
        text.ends_with("(Complete: 1 replacement in 1 file; 2 files skipped.)"),
        "{text}"
    );
    assert_eq!(std::fs::read(root.join("text.txt")).unwrap(), b"done\n");
    assert!(session.close().success());
}

#[test]
fn replace_errors_are_explicit_and_leave_files_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.txt");
    write(&path, b"one\n");
    let mut session = edit_session(temp.path(), None);

    let missing_path = session.call(
        "replace",
        serde_json::json!({"path":"", "pattern":"x", "replacement":"y"}),
    );
    assert_eq!(missing_path["result"]["isError"], true);
    assert_eq!(
        mcp_text(&missing_path),
        "The path parameter is required. Give the absolute file or directory to edit."
    );

    let huge = temp.path().join("over-64-mib.txt");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let too_large = session.call(
        "replace",
        serde_json::json!({
            "path":normalized(&huge), "pattern":"x", "replacement":"y"
        }),
    );
    assert_eq!(too_large["result"]["isError"], true);
    assert_eq!(
        mcp_text(&too_large),
        format!(
            "File too large for line edits: {} is 64.0 MiB (limit: 64 MiB).",
            normalized(&huge)
        )
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"one\n");
    assert!(session.close().success());

    let mut invalid_budget = edit_session(temp.path(), Some("broken"));
    let response = invalid_budget.call(
        "replace",
        serde_json::json!({
            "path":normalized(&path), "pattern":"one", "replacement":"two"
        }),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        mcp_text(&response),
        "Invalid FASTCTX_TOKEN_BUDGET value \"broken\": expected a positive integer. Fix the env var and restart the session."
    );
    assert!(invalid_budget.close().success());

    let mut tiny_budget = edit_session(temp.path(), Some("1"));
    let response = tiny_budget.call(
        "replace",
        serde_json::json!({
            "path":normalized(&path), "pattern":"missing", "replacement":"two"
        }),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        mcp_text(&response),
        "FASTCTX_TOKEN_BUDGET=1 is too small to return the required status note. Increase it and retry."
    );
    assert!(tiny_budget.close().success());
}

#[cfg(unix)]
#[test]
fn symlinks_are_preserved_hardlinks_are_refused_and_unix_mode_survives() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.txt");
    let link = temp.path().join("link.txt");
    write(&target, b"old\n");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &link).unwrap();
    let mut session = edit_session(temp.path(), None);
    replace_file(&mut session, &link, "old", "new", None);
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"new\n");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );

    let hard = temp.path().join("hard.txt");
    std::fs::hard_link(&target, &hard).unwrap();
    let response = session.call(
        "replace",
        serde_json::json!({"path":normalized(&target), "pattern":"new", "replacement":"bad"}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert!(mcp_text(&response).contains("it has multiple hard links"));
    assert_eq!(std::fs::read(&target).unwrap(), b"new\n");
    assert_eq!(std::fs::read(&hard).unwrap(), b"new\n");
    assert!(session.close().success());
}

fn edit_session(root: &Path, budget: Option<&str>) -> McpSession {
    McpSession::start(edit_command(root, budget, false))
}

fn edit_command(root: &Path, budget: Option<&str>, compatibility_flag: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command.arg("serve").current_dir(root);
    if compatibility_flag {
        command.arg("--enable-edit");
    }
    if let Some(budget) = budget {
        command.env("FASTCTX_TOKEN_BUDGET", budget);
    }
    #[cfg(unix)]
    {
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        command.env("XDG_RUNTIME_DIR", runtime);
    }
    #[cfg(windows)]
    command
        .env("LOCALAPPDATA", root)
        .env("TEMP", root)
        .env("TMP", root);
    command
}

fn replace_file(
    session: &mut McpSession,
    path: &Path,
    pattern: &str,
    replacement: &str,
    encoding: Option<&str>,
) {
    let mut arguments = serde_json::json!({
        "path":normalized(path), "pattern":pattern, "replacement":replacement
    });
    if let Some(encoding) = encoding {
        arguments["encoding"] = serde_json::Value::String(encoding.to_string());
    }
    let response = session.call("replace", arguments);
    assert_eq!(
        response["result"]["isError"],
        false,
        "{}",
        mcp_text(&response)
    );
}

fn encode_legacy(encoding: &'static encoding_rs::Encoding, text: &str) -> Vec<u8> {
    let (bytes, _, had_errors) = encoding.encode(text);
    assert!(!had_errors);
    bytes.into_owned()
}
