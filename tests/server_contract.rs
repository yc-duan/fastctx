mod common;

#[cfg(feature = "pdf")]
use common::write_pdf;
use common::{normalized, write};
use fastctx::server::{FastCtxServer, ServerOptions};
use rmcp::ServerHandler;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn default_tool_definitions_publish_replace_with_explicit_permissions() {
    let tools = FastCtxServer::new().tool_definitions();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["glob", "grep", "read", "replace"]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(
            annotations.read_only_hint,
            Some(tool.name != "replace"),
            "{}",
            tool.name
        );
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert!(tool.output_schema.is_none());
        assert!(tool.input_schema.get("type").is_some());
    }
    let read = tools.iter().find(|tool| tool.name == "read").unwrap();
    assert_eq!(
        read.description.as_deref(),
        Some(concat!(
            "Read one file (text, image, or PDF) or a batch of text files from the local\n",
            "filesystem. Text returns 1-based `N<tab>content` lines, 2000 per page; page\n",
            "with offset/limit. To read several text files in one call, pass\n",
            "files=[{\"path\": ...}, ...] (1-32 entries, each with optional offset, limit,\n",
            "and encoding) instead of file_path: contents are delivered in request order\n",
            "within one token budget, per-file problems are reported inline without\n",
            "failing the batch, and a Partial note returns the exact files array for the\n",
            "next call. Images (PNG/JPG/GIF/WebP/BMP) are shown to you visually. PDFs\n",
            "return the selected pages' text layer (pdf_mode=\"text\", default) or each\n",
            "page rendered as an image (pdf_mode=\"image\"). Text mode requires `pages`\n",
            "over 10 pages; image mode defaults to 4 pages. Max 20 pages per call.\n",
            "view=\"hex\" dumps any file's raw bytes. PDFs, images, and hex view are\n",
            "single-file only. Text output is always UTF-8; omit encoding for\n",
            "conservative auto-detection (BOM and valid UTF-8 are trusted, legacy text\n",
            "only after consistency checks) — if uncertain it returns an error listing\n",
            "candidate encodings instead of guessed text, so pass encoding (e.g. \"gbk\")\n",
            "only when you know the source encoding or a prior read reported ambiguity.\n",
            "Paths must be absolute. Text, PDF, and hex responses end with a Complete or\n",
            "Partial status — continue only with the exact parameters a Partial note\n",
            "provides. Plain images, warnings, and errors are self-contained."
        ))
    );
    assert!(
        read.input_schema
            .get("required")
            .is_none_or(|required| required.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(read.input_schema["properties"]["files"]["minItems"], 1);
    assert_eq!(read.input_schema["properties"]["files"]["maxItems"], 32);
    assert_eq!(
        read.input_schema["properties"]["files"]["items"]["$ref"],
        "#/$defs/BatchReadEntry"
    );
    assert_eq!(
        read.input_schema["$defs"]["BatchReadEntry"]["required"],
        serde_json::json!(["path"])
    );
    assert_eq!(
        read.input_schema["$defs"]["BatchReadEntry"]["properties"]["offset"]["minimum"],
        1
    );
    assert_eq!(
        read.input_schema["$defs"]["BatchReadEntry"]["properties"]["limit"]["minimum"],
        1
    );
    assert_eq!(read.input_schema["properties"]["offset"]["minimum"], 1);
    assert_eq!(read.input_schema["properties"]["limit"]["minimum"], 1);
    let pdf_mode_schema = read.input_schema["properties"]["pdf_mode"].to_string();
    assert!(pdf_mode_schema.contains("text"));
    assert!(pdf_mode_schema.contains("image"));
    assert!(read.input_schema["properties"].get("encoding").is_some());
    assert_eq!(
        read.input_schema["properties"]["encoding"]["description"],
        "Text files only. Known source encoding as a WHATWG label, e.g. \"gbk\", \"shift_jis\", \"big5\", \"euc-kr\", \"windows-1252\", \"utf-16le\", plus \"utf-32le\"/\"utf-32be\". Selects how source bytes are decoded; output is always UTF-8. Omit for auto-detection; set it when you know the source encoding or the tool reports an ambiguous or undecodable encoding."
    );
    let view_schema = read.input_schema["properties"]["view"].to_string();
    assert!(view_schema.contains("auto"));
    assert!(view_schema.contains("hex"));
    let grep = tools.iter().find(|tool| tool.name == "grep").unwrap();
    assert_eq!(
        grep.description.as_deref(),
        Some(concat!(
            "Fast regex content search (ripgrep engine; Rust regex, no lookaround). Output\n",
            "modes: \"files_with_matches\" (default, paths only), \"content\" (matching lines,\n",
            "optional context), \"count\" (per-file occurrence counts — total matches, not\n",
            "matching-line count), \"summary\" (global totals only).\n",
            "Respects .gitignore; searches hidden files; skips .git and binaries. Files are\n",
            "decoded to UTF-8 before searching; files whose encoding can't be determined, or\n",
            "that change during a directory search, are skipped and listed (never silently) —\n",
            "pass fallback_encoding (directory) or encoding (single file) to resolve encoding;\n",
            "a changing single-file target returns an error. Matching is line-by-line: `^` and\n",
            "`$` anchor line boundaries and are CRLF-aware. Set multiline=true for patterns\n",
            "spanning lines (`.` matches newlines; `\\n` also matches `\\r\\n`). A path component\n",
            "of the form ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~ (Windows\n",
            "UTF-16) is a filename escape; copy that whole component verbatim in later calls\n",
            "and do not decode or rewrite it. The last line of every successful result states\n",
            "Complete or Partial — continue only with the exact offset a Partial note provides;\n",
            "errors are self-contained."
        ))
    );
    assert_eq!(
        grep.input_schema["required"],
        serde_json::json!(["pattern"])
    );
    assert!(grep.input_schema["properties"].get("type").is_some());
    assert!(grep.input_schema["properties"].get("file_type").is_none());
    assert_eq!(
        grep.input_schema["properties"]["encoding"]["description"],
        "Single-file target only: decode that file with this WHATWG encoding label (e.g. \"gbk\"), same semantics as read's encoding. On a directory target use fallback_encoding instead."
    );
    assert_eq!(
        grep.input_schema["properties"]["fallback_encoding"]["description"],
        "Directory target: WHATWG encoding to assume only for files auto-detection can't determine — never overrides BOM, valid UTF-8, or already-resolved files. Strict-decoded; files that also fail under it stay in the skip report."
    );
    let output_mode_schema = grep.input_schema["properties"]["output_mode"].to_string();
    for mode in ["content", "files_with_matches", "count", "summary"] {
        assert!(output_mode_schema.contains(mode), "{output_mode_schema}");
    }
    let glob = tools.iter().find(|tool| tool.name == "glob").unwrap();
    assert_eq!(
        glob.description.as_deref(),
        Some(concat!(
            "Find files by glob pattern, e.g. \"**/*.rs\" or \"src/**/*.ts\". Returns absolute\n",
            "paths sorted by path (or newest first with sort=\"modified\"), 100 per page.\n",
            "filter_mode \"project\" (default) respects .gitignore and skips .git;\n",
            "filter_mode \"all\" lists everything. Omit `path` to search the session working\n",
            "directory — omit the field entirely, never pass \"null\" or \"undefined\". A path\n",
            "component of the form ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~\n",
            "(Windows UTF-16) is a filename escape; copy that whole component verbatim in\n",
            "later calls and do not decode or rewrite it. The last line of every successful\n",
            "result states Complete or Partial — continue only with the exact offset a Partial\n",
            "note provides; errors are self-contained."
        ))
    );
    assert_eq!(
        glob.input_schema["required"],
        serde_json::json!(["pattern"])
    );
    for property in ["filter_mode", "sort", "offset", "limit"] {
        assert!(glob.input_schema["properties"].get(property).is_some());
    }
    assert_eq!(glob.input_schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(glob.input_schema["properties"]["limit"]["maximum"], 1_000);
    let descriptions = tools
        .iter()
        .map(|tool| tool.description.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    for keyword in ["file", "read", "grep", "search", "glob", "replace"] {
        assert!(descriptions.to_ascii_lowercase().contains(keyword));
    }
}

#[test]
fn server_instructions_follow_the_optional_shell_group() {
    for enable_shell in [false, true] {
        let info = FastCtxServer::with_options(ServerOptions { enable_shell }).get_info();
        let instructions = info.instructions.as_deref().unwrap();
        assert_eq!(
            instructions.contains("POSIX-bash"),
            enable_shell,
            "{instructions}"
        );
        assert!(instructions.contains("replace"), "{instructions}");
        for removed in ["named clips", "copy", "cut", "paste"] {
            assert!(!instructions.contains(removed), "{instructions}");
        }
    }
}

#[test]
fn all_nine_tools_publish_explicit_three_hint_annotations() {
    let tools = FastCtxServer::with_options(ServerOptions::all()).tool_definitions();
    assert_eq!(tools.len(), 9);

    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert!(annotations.read_only_hint.is_some(), "{}", tool.name);
        assert_eq!(annotations.destructive_hint, Some(false), "{}", tool.name);
        assert_eq!(annotations.open_world_hint, Some(false), "{}", tool.name);
    }
    for name in ["glob", "grep", "job_list", "job_output", "read"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
    }
    for name in ["run", "run_background", "job_kill", "replace"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(false)
        );
    }
}

#[test]
fn shell_and_replace_tool_descriptions_and_schemas_match_the_frozen_contract() {
    let tools = FastCtxServer::with_options(ServerOptions::all()).tool_definitions();
    let shell = tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.name.as_ref(),
                "job_kill" | "job_list" | "job_output" | "run" | "run_background"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shell
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        [
            "job_kill",
            "job_list",
            "job_output",
            "run",
            "run_background",
        ]
    );
    let run = shell.iter().find(|tool| tool.name == "run").unwrap();
    assert_eq!(
        run.description.as_deref(),
        Some(concat!(
            "Run a shell command with bash (Git Bash on Windows; system bash elsewhere)\n",
            "and return its merged stdout+stderr with the exit code. Write POSIX bash —\n",
            "never PowerShell. Commands must be non-interactive: there is no TTY or\n",
            "stdin; use flags like -y or --no-edit. A non-zero exit code is a normal\n",
            "result, not an error. Oversized output is truncated (with a note); to get\n",
            "the full output, redirect it to a file (command > file 2>&1) and page that\n",
            "file with the read tool. Default timeout 120000 ms (max 240000) — start\n",
            "anything longer with run_background. cwd must be absolute when given.\n",
            "If the output looks garbled (U+FFFD), pass encoding with the source\n",
            "encoding label (e.g. \"gbk\"). ",
            "The last line states Complete or Partial."
        ))
    );
    assert_eq!(run.input_schema["required"], serde_json::json!(["command"]));
    assert_eq!(run.input_schema["properties"]["timeout_ms"]["minimum"], 1);
    assert_eq!(
        run.input_schema["properties"]["timeout_ms"]["maximum"],
        240_000
    );
    assert_eq!(
        run.input_schema["properties"]["login_shell"]["default"],
        true
    );
    assert!(run.input_schema["properties"].get("encoding").is_some());
    let background = shell
        .iter()
        .find(|tool| tool.name == "run_background")
        .unwrap();
    assert_eq!(
        background.description.as_deref(),
        Some(concat!(
            "Start a bash command as a background job and return its job_id\n",
            "immediately. Use for builds, tests, servers, or anything that may exceed\n",
            "two minutes. Jobs run independently of this session: they survive server\n",
            "and Codex restarts, and their output and exit code stay retrievable by\n",
            "job_id afterwards. Check on it with job_output; stop with job_kill;\n",
            "rediscover past jobs with job_list. There is no timeout — a job runs\n",
            "until it exits or is killed. Everything the job prints is also kept in a\n",
            "plain log file whose path is returned here: read or grep it with the read\n",
            "tool for anything job_output does not show."
        ))
    );
    assert_eq!(
        background.input_schema["required"],
        serde_json::json!(["command"])
    );
    assert!(
        background.input_schema["properties"]
            .get("timeout_ms")
            .is_none()
    );
    assert_eq!(
        background.input_schema["properties"]["login_shell"]["default"],
        true
    );
    assert!(
        background.input_schema["properties"]
            .get("encoding")
            .is_some()
    );
    let output = shell.iter().find(|tool| tool.name == "job_output").unwrap();
    assert_eq!(
        output.description.as_deref(),
        Some(concat!(
            "Query a background job: its status (running, exited with its code, or\n",
            "interrupted) plus the newest output you have not been shown yet. wait_ms\n",
            "is how long this query may take (0-60000, default 30000): it returns as\n",
            "soon as the job reaches a terminal state, and otherwise waits the window\n",
            "out — intermediate output does not end the wait, so one call is worth one\n",
            "turn. Pass wait_ms=0 for an immediate snapshot. Long output is windowed:\n",
            "you get the newest lines that fit, plus the start of the log on the first\n",
            "call, and a note naming the exact lines that were skipped. Nothing is\n",
            "lost — the job's whole output is a plain log file on disk, and its line\n",
            "numbers are the seq numbers used here, so read or grep that path for\n",
            "anything not shown. Works for jobs started in earlier sessions. If output\n",
            "looks garbled (U+FFFD), call again with encoding set to the source\n",
            "encoding (e.g. \"gbk\") — stored bytes are re-decoded losslessly. Keep\n",
            "calling until the last line says Complete."
        ))
    );
    assert_eq!(
        output.input_schema["required"],
        serde_json::json!(["job_id"])
    );
    assert_eq!(output.input_schema["properties"]["wait_ms"]["minimum"], 0);
    assert_eq!(
        output.input_schema["properties"]["wait_ms"]["maximum"],
        60_000
    );
    assert_eq!(
        output.input_schema["properties"]["wait_ms"]["default"],
        30_000
    );
    assert!(output.input_schema["properties"].get("wait_for").is_none());
    assert_eq!(output.input_schema["properties"]["after_seq"]["minimum"], 0);
    assert!(output.input_schema["properties"].get("encoding").is_some());
    let list = shell.iter().find(|tool| tool.name == "job_list").unwrap();
    assert_eq!(
        list.description.as_deref(),
        Some(concat!(
            "List background jobs across all FastCtx sessions for the current user.\n",
            "status defaults to running; use finished to inspect exited or interrupted\n",
            "records, or all only when both lifecycles are needed. Results are newest\n",
            "first within each lifecycle. limit defaults to the current-user\n",
            "fastshell.job_list_limit setting (20 initially, maximum 100), and offset\n",
            "continues a page. Finished records remain available until the job storage\n",
            "limit evicts the oldest."
        ))
    );
    assert!(
        list.input_schema
            .get("required")
            .is_none_or(|required| required.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(
        list.input_schema["properties"]["status"]["$ref"],
        "#/$defs/JobListStatus"
    );
    assert_eq!(
        list.input_schema["$defs"]["JobListStatus"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["const"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["running", "finished", "all"]
    );
    assert_eq!(list.input_schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(list.input_schema["properties"]["limit"]["maximum"], 100);
    assert_eq!(list.input_schema["properties"]["offset"]["minimum"], 0);

    let replace = tools.iter().find(|tool| tool.name == "replace").unwrap();
    assert_eq!(
        replace.description.as_deref(),
        Some(concat!(
            "Batch find-and-replace across a file or directory (Rust regex, same engine\n",
            "as grep; no lookaround). replacement supports $1/${name} groups, $$ for a\n",
            "literal $; a reference to an undefined capture group is rejected before any\n",
            "write; an empty replacement deletes the match (include \\n in the\n",
            "pattern to delete whole lines). Matching is leftmost-first and\n",
            "non-overlapping; unlike grep, `^`/`$` anchor the whole file by default —\n",
            "use (?m) for per-line anchors. Respects .gitignore; skips .git, binaries, and files\n",
            "whose encoding cannot be determined (listed, never silent). Each file is\n",
            "written atomically with a concurrent-modification check, preserving its\n",
            "original encoding, BOM, and line endings. path is required. Set\n",
            "dry_run=true to preview; set max_replacements to cap the blast radius. The\n",
            "last line states Complete or Partial."
        ))
    );
    assert_eq!(
        replace.input_schema["required"],
        serde_json::json!(["pattern", "replacement", "path"])
    );
    for property in [
        "glob",
        "literal",
        "case_insensitive",
        "dot_all",
        "max_replacements",
        "dry_run",
        "encoding",
        "fallback_encoding",
    ] {
        assert!(
            replace.input_schema["properties"].get(property).is_some(),
            "{property}"
        );
    }
}

#[test]
fn stdio_glob_uses_the_server_working_directory_when_path_is_omitted() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("cwd.txt");
    write(&file, b"cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cwd-test", "version": "1.0"}
            }
        }),
    );
    let _ = read_response(&mut stdout);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"glob","arguments":{"pattern":"*.txt"}}
        }),
    );
    let response = read_response(&mut stdout);
    assert_eq!(response["result"]["isError"], false);
    assert!(response["result"].get("structuredContent").is_none());
    assert_eq!(
        response["result"]["content"][0]["text"],
        format!("{}\n\n(Complete: all 1 file shown.)", normalized(&file))
    );

    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn non_pdf_stdio_calls_do_not_extract_the_bundled_engine() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain");
    let cache_root = temp.path().join("cache-root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let expected_engine_dir = configure_isolated_cache(&mut command, &cache_root);
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "lazy-test", "version": "1.0"}
            }
        }),
    );
    let _ = read_response(&mut stdout);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"read","arguments":{"file_path":normalized(&file)}}
        }),
    );
    let response = read_response(&mut stdout);
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "1\tplain\n\n(Complete: reached end of file; line 1 of 1 shown.)"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
    if expected_engine_dir.exists() {
        let direct_files = std::fs::read_dir(&expected_engine_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>();
        assert!(
            direct_files.is_empty(),
            "a non-PDF call extracted cache files: {direct_files:?}"
        );
    }
}

#[test]
#[cfg(feature = "pdf")]
fn stdio_pdf_call_extracts_one_hashed_engine_and_preserves_image_meta() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("page.pdf");
    write_pdf(&pdf, &[Some("MCP PDF one"), Some("MCP PDF two")]);
    let cache_root = temp.path().join("cache-root");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let engine_dir = configure_isolated_cache(&mut command, &cache_root);
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "pdf-test", "version": "1.0"}
            }
        }),
    );
    let _ = read_response(&mut stdout);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"read","arguments":{"file_path":normalized(&pdf),"pdf_mode":"image"}}
        }),
    );
    let response = read_response(&mut stdout);
    assert_eq!(response["result"]["isError"], false);
    assert!(response["result"].get("structuredContent").is_none());
    assert_eq!(response["result"]["content"].as_array().unwrap().len(), 3);
    assert_eq!(response["result"]["content"][0]["type"], "image");
    assert_eq!(response["result"]["content"][1]["type"], "image");
    assert_eq!(response["result"]["content"][2]["type"], "text");
    assert_eq!(
        response["result"]["content"][2]["text"],
        "(Complete: pages 1-2 of 2 rendered.)"
    );
    for image_index in [0, 1] {
        assert_eq!(
            response["result"]["content"][image_index]["_meta"]["codex/imageDetail"],
            "high"
        );
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());

    let released = std::fs::read_dir(&engine_dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry.file_type().unwrap().is_file()
                && !entry.file_name().to_string_lossy().ends_with(".lock")
        })
        .collect::<Vec<_>>();
    assert_eq!(released.len(), 1, "{released:?}");
    let name = released[0].file_name().to_string_lossy().into_owned();
    assert!(name.contains("chromium-7763"));
    assert!(released[0].metadata().unwrap().len() > 1_000_000);
}

#[test]
fn stdio_mcp_lists_tools_and_never_returns_structured_content() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = BufReader::new(stdout);

    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "1.0"}
            }
        }),
    );
    let initialized = read_response(&mut stdout);
    assert_eq!(initialized["id"], 1);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let listed = read_response(&mut stdout);
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 4);

    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{"name":"read","arguments":{"file_path":"Z:/definitely/missing.txt"}}
        }),
    );
    let called = read_response(&mut stdout);
    assert_eq!(called["result"]["isError"], true);
    assert!(called["result"].get("structuredContent").is_none());
    assert_eq!(called["result"]["content"][0]["type"], "text");

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
fn stdio_serve_flags_publish_exact_four_and_nine_tool_sets() {
    let cases: [(&[&str], &[&str]); 4] = [
        (&["serve"], &["glob", "grep", "read", "replace"]),
        (
            &["serve", "--enable-shell"],
            &[
                "glob",
                "grep",
                "job_kill",
                "job_list",
                "job_output",
                "read",
                "replace",
                "run",
                "run_background",
            ],
        ),
        (
            &["serve", "--enable-edit"],
            &["glob", "grep", "read", "replace"],
        ),
        (
            &["serve", "--enable-shell", "--enable-edit"],
            &[
                "glob",
                "grep",
                "job_kill",
                "job_list",
                "job_output",
                "read",
                "replace",
                "run",
                "run_background",
            ],
        ),
    ];

    for (args, expected) in cases {
        assert_eq!(list_tool_names(args), expected, "args={args:?}");
    }
}

#[test]
fn stdio_head_limit_zero_still_uses_the_environment_token_budget() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("many.txt");
    write(&file, "hit\n".repeat(100));
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command.env("FASTCTX_TOKEN_BUDGET", "30");
    let response = call_tool(
        command,
        "grep",
        serde_json::json!({
            "pattern": "hit",
            "path": normalized(&file),
            "output_mode": "content",
            "head_limit": 0
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "1:hit\n2:hit\n\n(Partial: results 1-2 shown; more exist. Continue with offset=2.)"
    );
}

#[test]
fn stdio_preserves_utf8_text_without_host_codepage_transcoding() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("unicode.txt");
    write(&file, "alpha\n中文 sentinel\n".as_bytes());
    let response = call_tool(
        Command::new(env!("CARGO_BIN_EXE_fastctx")),
        "read",
        serde_json::json!({"file_path": normalized(&file)}),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "1\talpha\n2\t中文 sentinel\n3\t\n\n(Complete: reached end of file; lines 1-3 of 3 shown.)"
    );
}

#[test]
fn stdio_invalid_token_budget_is_an_exact_tool_error() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command.env("FASTCTX_TOKEN_BUDGET", "0");
    let response = call_tool(
        command,
        "read",
        serde_json::json!({"file_path": normalized(&file)}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Invalid FASTCTX_TOKEN_BUDGET value \"0\": expected a positive integer."
    );
}

#[test]
fn stdio_batch_read_requires_room_for_one_line_and_its_exact_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain\nmore");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .env("FASTCTX_TOKEN_BUDGET", "10")
        .env("FASTCTX_READ_TOKEN_BUDGET", "1");
    let response = call_tool(
        command,
        "read",
        serde_json::json!({"files": [{"path": normalized(&file)}]}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "FASTCTX_READ_TOKEN_BUDGET=1 is too small to return the required continuation note. Increase it and retry."
    );
}

#[test]
fn stdio_per_tool_budgets_must_not_exceed_the_global_budget() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain");
    let cases = [
        (
            "read",
            "FASTCTX_READ_TOKEN_BUDGET",
            serde_json::json!({"file_path": normalized(&file)}),
        ),
        (
            "grep",
            "FASTCTX_GREP_TOKEN_BUDGET",
            serde_json::json!({"pattern": "plain", "path": normalized(&file)}),
        ),
        (
            "glob",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            serde_json::json!({"pattern": "*.txt", "path": normalized(temp.path())}),
        ),
    ];

    for (tool, variable, arguments) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
        command
            .env("FASTCTX_TOKEN_BUDGET", "100")
            .env(variable, "101");
        let response = call_tool(command, tool, arguments);
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["content"][0]["text"],
            format!(
                "{variable}=101 exceeds FASTCTX_TOKEN_BUDGET=100. Increase the global budget or lower the per-tool budget."
            )
        );
    }
}

#[test]
fn stdio_per_tool_budgets_reject_non_positive_values() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain");
    let cases = [
        (
            "read",
            "FASTCTX_READ_TOKEN_BUDGET",
            serde_json::json!({"file_path": normalized(&file)}),
        ),
        (
            "grep",
            "FASTCTX_GREP_TOKEN_BUDGET",
            serde_json::json!({"pattern": "plain", "path": normalized(&file)}),
        ),
        (
            "glob",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            serde_json::json!({"pattern": "*.txt", "path": normalized(temp.path())}),
        ),
    ];

    for (tool, variable, arguments) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
        command.env(variable, "0");
        let response = call_tool(command, tool, arguments);
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["content"][0]["text"],
            format!("Invalid {variable} value \"0\": expected a positive integer.")
        );
    }
}

#[test]
#[cfg(feature = "pdf")]
fn stdio_pdf_text_mode_uses_the_read_specific_page_budget() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("budget.pdf");
    let long_page = "x".repeat(5_000);
    write_pdf(&pdf, &[Some("Small"), Some(long_page.as_str())]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command.env("FASTCTX_READ_TOKEN_BUDGET", "34");
    let response = call_tool(
        command,
        "read",
        serde_json::json!({"file_path": normalized(&pdf), "pages": "1-2"}),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "=== Page 1 ===\nSmall\n\n(Partial: page 1 of 2 shown. Continue with pages=\"2\".)"
    );
}

#[test]
#[cfg(feature = "pdf")]
fn stdio_pdf_call_repairs_a_corrupted_cached_engine() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("page.pdf");
    write_pdf(&pdf, &[Some("Cache repair")]);
    let cache_root = temp.path().join("cache-root");

    let mut first_command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    let engine_dir = configure_isolated_cache(&mut first_command, &cache_root);
    let first = call_tool(
        first_command,
        "read",
        serde_json::json!({"file_path": normalized(&pdf)}),
    );
    assert_eq!(first["result"]["isError"], false);
    let engine = std::fs::read_dir(&engine_dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            entry.file_type().unwrap().is_file()
                && !entry.file_name().to_string_lossy().ends_with(".lock")
        })
        .unwrap()
        .path();
    let original = std::fs::read(&engine).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&engine, b"corrupted").unwrap();

    let mut second_command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    configure_isolated_cache(&mut second_command, &cache_root);
    let second = call_tool(
        second_command,
        "read",
        serde_json::json!({"file_path": normalized(&pdf)}),
    );
    assert_eq!(second["result"]["isError"], false);
    assert_eq!(std::fs::read(engine).unwrap(), original);
}

#[test]
#[cfg(not(feature = "pdf"))]
fn no_pdf_build_rejects_pdf_without_affecting_the_public_read_schema() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("disabled.pdf");
    write(&pdf, b"%PDF-1.4\n");
    let response = call_tool(
        Command::new(env!("CARGO_BIN_EXE_fastctx")),
        "read",
        serde_json::json!({"file_path": normalized(&pdf)}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["content"][0]["text"],
        "PDF support is unavailable: could not load the bundled PDF engine (this binary was built without the pdf feature). Other file types are unaffected."
    );
}

fn send(stdin: &mut impl Write, value: Value) {
    writeln!(stdin, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    stdin.flush().unwrap();
}

fn read_response(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn list_tool_names(args: &[&str]) -> Vec<String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "tool-list-contract", "version": "1.0"}
            }
        }),
    );
    let initialized = read_response(&mut stdout);
    assert_eq!(initialized["id"], 1);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let listed = read_response(&mut stdout);
    let mut names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    names.sort();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    names
}

fn call_tool(mut command: Command, name: &str, arguments: Value) -> Value {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "helper", "version": "1.0"}
            }
        }),
    );
    let _ = read_response(&mut stdout);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }),
    );
    let response = read_response(&mut stdout);
    drop(stdin);
    assert!(child.wait().unwrap().success());
    response
}

fn configure_isolated_cache(command: &mut Command, root: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        command.env("LOCALAPPDATA", root);
        root.join("fastctx")
    }
    #[cfg(target_os = "macos")]
    {
        command.env("HOME", root);
        root.join("Library/Caches/fastctx")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        command.env("XDG_CACHE_HOME", root);
        root.join("fastctx")
    }
}
