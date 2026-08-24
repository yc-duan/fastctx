mod common;

#[cfg(feature = "pdf")]
use common::write_pdf;
use common::{normalized, write};
use fastctx::server::{FastCtxServer, ServerOptions};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// One temporary profile shared by every server this binary spawns.
///
/// An inherited HOME resolves the control-center endpoint, the Codex profile, and the provider
/// guard from the developer's real machine. A third-party provider there activates Guarded mode,
/// which silently rewrites the budget variables these tests set. CI images have no Codex profile,
/// so the failure only ever appears on a developer machine.
fn isolated_home() -> &'static std::path::Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| tempfile::tempdir().expect("a temporary profile for the test servers"))
        .path()
}

/// Spawns the MCP binary with the control-center idle timeout shared by the test tree.
fn fastctx_command() -> Command {
    fastctx_command_for_home(isolated_home())
}

fn fastctx_command_for_home(home: &std::path::Path) -> Command {
    std::fs::create_dir_all(home).expect("the isolated server profile should be creatable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", common::TEST_HOST_IDLE_MS)
        .env("HOME", home)
        .env("USERPROFILE", home);
    command
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
    for name in [
        "glob",
        "grep",
        "job_list",
        "job_output",
        "inspect_local_file",
    ] {
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

/// Constructs that make a provider reject the whole tool declaration, each paired with
/// the failure it causes so a reintroduction is diagnosed rather than merely flagged.
///
/// These are not style preferences. A tool declaration is validated before the request
/// is submitted, so one rejected keyword returns 400 for the entire turn and takes every
/// FastCtx tool down with it — the symptom is "FastCtx does not work on this provider",
/// not "one parameter behaves oddly".
const REJECTED_SCHEMA_KEYWORDS: [(&str, &str); 9] = [
    (
        "$ref",
        "the Gemini API Schema type has no $ref field, and Codex rewrites every local \
         $ref to {} once a tool schema passes its compaction budget, silently erasing \
         an enum parameter's accepted values",
    ),
    (
        "$defs",
        "a definition table is unreachable once $ref is gone, and providers that do not \
         know the key reject it",
    ),
    (
        "oneOf",
        "absent from the Gemini, OpenAI and Anthropic keyword sets; a fieldless enum \
         belongs in type: \"string\" plus enum",
    ),
    (
        "anyOf",
        "Gemini rejects an anyOf node that carries any sibling key, and every parameter \
         here carries a description; express the union at the Rust type instead",
    ),
    (
        "allOf",
        "OpenAI lists allOf as unsupported and the Gemini Schema type has no such field",
    ),
    (
        "const",
        "no provider subset accepts it; a string const belongs in enum",
    ),
    (
        "$schema",
        "metadata no consumer reads, and every known host transform strips it",
    ),
    (
        "additionalProperties",
        "absent from the Gemini API Schema type, where an unrecognized key is an \
         Unknown name 400; serde(deny_unknown_fields) still enforces this at call time",
    ),
    (
        "format",
        "the derived numeric widths (uint, uint64, int64) are outside every provider's \
         accepted format set, and minimum/maximum already carry the bound",
    ),
];

/// Keywords a published input schema may use.
///
/// Deliberately an allow-list. schemars gains keywords over time and providers reject
/// what they do not recognize, so a newly emitted keyword must fail here and be argued
/// for rather than ship unnoticed. Every entry appears in the Gemini API Schema type,
/// OpenAI's supported-keyword set, and Anthropic's.
const PORTABLE_SCHEMA_KEYWORDS: [&str; 11] = [
    "type",
    "description",
    "properties",
    "required",
    "items",
    "enum",
    "default",
    "minimum",
    "maximum",
    "minItems",
    "maxItems",
];

/// Scalar type names a node may carry. "null" is absent on purpose: optionality is
/// carried by `required`, and Gemini's schema type is a single scalar with no way to
/// spell a nullable union.
const PORTABLE_SCHEMA_TYPES: [&str; 6] =
    ["object", "array", "string", "integer", "number", "boolean"];

/// Freezes the portable subset across every published tool rather than a named list of
/// parameters, so a new tool, a new parameter, or a new schemars version cannot
/// reintroduce a construct that costs us a whole provider. `src/tool_schema.rs` states
/// the subset and the per-keyword reasoning; read it before widening either list above.
#[test]
fn published_tool_schemas_stay_inside_the_portable_subset() {
    for tool in FastCtxServer::with_options(ServerOptions::all()).tool_definitions() {
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(
            schema["type"], "object",
            "{}: a tool's parameters must be an object",
            tool.name
        );
        assert_portable_schema_node(&schema, &tool.name);
    }
}

fn assert_portable_schema_node(node: &Value, path: &str) {
    let map = node
        .as_object()
        .unwrap_or_else(|| panic!("{path}: every published schema node must be an object: {node}"));
    for (keyword, reason) in REJECTED_SCHEMA_KEYWORDS {
        assert!(
            !map.contains_key(keyword),
            "{path}: `{keyword}` reappeared in a published tool schema — {reason}"
        );
    }
    for key in map.keys() {
        assert!(
            PORTABLE_SCHEMA_KEYWORDS.contains(&key.as_str()),
            "{path}: `{key}` is outside the portable keyword set; \
             adding it means answering why every provider accepts it"
        );
    }
    let declared = map.get("type").and_then(Value::as_str).unwrap_or_else(|| {
        panic!(
            "{path}: needs one scalar `type`; a missing or list-valued type is exactly \
             what strict providers reject: {node}"
        )
    });
    assert!(
        PORTABLE_SCHEMA_TYPES.contains(&declared),
        "{path}: unsupported type `{declared}`"
    );
    if let Some(properties) = map.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            assert_portable_schema_node(property, &format!("{path}.{name}"));
        }
    }
    if let Some(items) = map.get("items") {
        assert_portable_schema_node(items, &format!("{path}[]"));
    }
}

/// The glob-pattern parameters advertise a plain string array while their Rust type
/// still accepts a bare string. Both forms must keep parsing: the array is what models
/// send once they read the published schema, the bare string is what every already
/// deployed caller sends.
#[test]
fn glob_pattern_parameters_accept_both_the_advertised_array_and_a_bare_string() {
    for pattern in [
        serde_json::json!(["**/*.rs", "!tests/**"]),
        "**/*.rs".into(),
    ] {
        serde_json::from_value::<fastctx::glob_tool::GlobRequest>(serde_json::json!({
            "pattern": pattern,
        }))
        .unwrap_or_else(|error| panic!("glob.pattern rejected {pattern}: {error}"));
        serde_json::from_value::<fastctx::grep_tool::GrepRequest>(serde_json::json!({
            "pattern": "needle",
            "glob": pattern,
        }))
        .unwrap_or_else(|error| panic!("grep.glob rejected {pattern}: {error}"));
        serde_json::from_value::<fastctx::edit::ReplaceRequest>(serde_json::json!({
            "pattern": "needle",
            "replacement": "thread",
            "path": "/tmp/file.txt",
            "glob": pattern,
        }))
        .unwrap_or_else(|error| panic!("replace.glob rejected {pattern}: {error}"));
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
            "Use for non-interactive local CLI work, including Git, build/test tools,\n",
            "package managers, database CLIs, and project scripts. Commands run with bash\n",
            "(Git Bash on Windows; system bash elsewhere) and return merged stdout+stderr\n",
            "with the exit code. Write POSIX bash — never PowerShell. There is no TTY or\n",
            "stdin; use flags like -y or --no-edit. A non-zero exit code is a normal\n",
            "result, not an error. Oversized output is truncated; for the full output,\n",
            "redirect it to a file (command > file 2>&1) and page that file with\n",
            "inspect_local_file.\n",
            "Default timeout 120000 ms, ceiling 240000 — start anything that may outlast\n",
            "it with run_background. If output looks garbled (U+FFFD), pass encoding\n",
            "(e.g. \"gbk\"). The last line states Complete, Partial, or Killed."
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
            "immediately. Use for builds, tests, servers, or anything that may outlast\n",
            "run's four-minute maximum. Jobs survive server and Codex restarts; their\n",
            "output and exit code stay retrievable by job_id. Check on it with\n",
            "job_output; stop with job_kill; rediscover past jobs with job_list. There\n",
            "is no timeout: a job runs until it exits or is killed. Everything it\n",
            "prints is kept in a plain log file whose path is returned here;\n",
            "inspect_local_file or grep that path for anything job_output does not show. While your jobs\n",
            "run, every FastCtx result carries a one-line background status naming\n",
            "each job and how long it has run, just above the closing Complete or\n",
            "Partial line. It is a readout, not a notification: it refreshes only when\n",
            "you call a tool, so keep working — nothing reaches you if you stop."
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
            "Query a background job: its status (running, exited with its code,\n",
            "killed, or interrupted) plus output you have not been shown yet. Works for jobs\n",
            "started in earlier sessions. Long output is windowed: the newest lines\n",
            "that fit, the start of the log on the first call, and a note naming the\n",
            "exact lines skipped. The job's whole output is a plain log file on disk\n",
            "whose line numbers are the seq numbers used here, so inspect_local_file or\n",
            "grep that path for anything not shown. The call blocks up to wait_ms, so raise it\n",
            "only when you have nothing else to do. If output looks garbled (U+FFFD),\n",
            "call again with encoding set to the source encoding (e.g. \"gbk\").\n",
            "Complete appears only once the job ends; servers and watchers never reach\n",
            "it. Take what you need and keep working — the background status on your\n",
            "next result carries this job's state."
        ))
    );
    // A running job is always Partial, and a dev server or watch never reaches a terminal
    // state — telling the caller to poll until Complete would prescribe an endless loop
    // (2026-07-24).
    assert!(
        !output
            .description
            .as_deref()
            .unwrap_or_default()
            .contains("until the last line says Complete"),
        "job_output must not prescribe polling to a terminal state"
    );
    assert_eq!(
        output.input_schema["required"],
        serde_json::json!(["job_id"])
    );
    assert_eq!(output.input_schema["properties"]["wait_ms"]["minimum"], 0);
    assert_eq!(
        output.input_schema["properties"]["wait_ms"]["maximum"],
        240_000
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
            "List background jobs across all FastCtx sessions for the current user. Use\n",
            "status=\"all\" only when both lifecycles are needed. Results are newest first\n",
            "within each lifecycle. Finished records remain available until the job\n",
            "storage limit evicts the oldest."
        ))
    );
    assert!(
        list.input_schema
            .get("required")
            .is_none_or(|required| required.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(list.input_schema["properties"]["status"]["type"], "string");
    assert_eq!(
        list.input_schema["properties"]["status"]["enum"],
        serde_json::json!(["running", "finished", "all"])
    );
    assert!(
        list.input_schema.get("$defs").is_none(),
        "enum variants are inlined, so no tool carries a definition table"
    );
    assert_eq!(list.input_schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(list.input_schema["properties"]["limit"]["maximum"], 100);
    assert_eq!(list.input_schema["properties"]["offset"]["minimum"], 0);

    let replace = tools.iter().find(|tool| tool.name == "replace").unwrap();
    assert_eq!(
        replace.description.as_deref(),
        Some(concat!(
            "Batch find-and-replace across a file or directory (Rust regex, same engine\n",
            "as grep; no lookaround). A reference to an undefined capture group is\n",
            "rejected before any write. To delete whole lines, include \\n in the\n",
            "pattern. Matching is leftmost-first and non-overlapping; unlike grep,\n",
            "`^`/`$` anchor the whole file by default — use (?m) for per-line anchors.\n",
            "Respects .gitignore; skips .git and binaries; files whose encoding cannot\n",
            "be determined are skipped and listed. Each file is written atomically with\n",
            "a concurrent-modification check, preserving its original encoding, BOM, and\n",
            "line endings. The last line states Complete or Partial."
        ))
    );

    let descriptions = tools
        .iter()
        .filter_map(|tool| tool.description.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    let job_notes = format!(
        "{}\n{}",
        include_str!("../src/shell/output.rs"),
        include_str!("../src/shell/jobs/mod.rs")
    );
    for forbidden in [
        "will be told",
        "will be notified",
        "wait for it to tell you",
        "notify you when",
        "tell you when it",
        "wait until notified",
    ] {
        assert!(
            !descriptions.to_ascii_lowercase().contains(forbidden),
            "tool descriptions must not promise a push notification: {forbidden}"
        );
        assert!(
            !job_notes.to_ascii_lowercase().contains(forbidden),
            "running and terminal notes must not promise push delivery: {forbidden}"
        );
    }
    assert!(
        !job_notes.to_ascii_lowercase().contains("notification"),
        "only the run_background description may use notification, and only in its explicit negation"
    );
    assert!(descriptions.contains("It is a readout, not a notification"));
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
    assert_positive_local_path_schema(&replace.input_schema["properties"]["path"]);
}

fn assert_positive_local_path_schema(schema: &Value) {
    let description = schema["description"]
        .as_str()
        .expect("path schema should describe the accepted local path shape");
    for required in [
        "Plain absolute local filesystem path",
        "URI-shaped",
        "equivalent local absolute path",
    ] {
        assert!(description.contains(required), "{required}: {description}");
    }
    for forbidden in [
        "read_mcp_resource",
        "list_mcp_resources",
        "list_mcp_resource_templates",
        "MCP resources",
        "file://",
        "FastCtx is not",
    ] {
        assert!(
            !description.contains(forbidden),
            "{forbidden}: {description}"
        );
    }
}

#[test]
fn non_pdf_stdio_calls_do_not_extract_the_bundled_engine() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("plain.txt");
    write(&file, b"plain");
    let cache_root = temp.path().join("cache-root");
    let home = temp.path().join("home");
    let mut command = fastctx_command_for_home(&home);
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
            "params":{"name":"inspect_local_file","arguments":{"file_path":normalized(&file)}}
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
fn stdio_mcp_is_tool_only_lists_tools_and_never_returns_structured_content() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = fastctx_command()
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
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    assert!(
        initialized["result"]["capabilities"]
            .get("resources")
            .is_none(),
        "{initialized}"
    );
    let instructions = initialized["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("Local-file tools"), "{instructions}");
    assert!(!instructions.contains("MCP resources"), "{instructions}");
    assert!(instructions.chars().count() <= 250, "{instructions}");
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
            "params":{"name":"inspect_local_file","arguments":{"file_path":"Z:/definitely/missing.txt"}}
        }),
    );
    let called = read_response(&mut stdout);
    assert_eq!(called["result"]["isError"], true);
    assert!(called["result"].get("structuredContent").is_none());
    assert_eq!(called["result"]["content"][0]["type"], "text");

    // Both discovery methods keep the rmcp default: an empty list, which answers "this server
    // has none" without failing. Rejecting them instead (0.2.2) made every misrouted call a
    // failure, and a failed call makes models retry with a different `server` argument rather
    // than switch tools, which is the chain of invented server names users hit (2026-08-01).
    for (id, method, key) in [
        (4, "resources/list", "resources"),
        (5, "resources/templates/list", "resourceTemplates"),
    ] {
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}),
        );
        let listed = read_response(&mut stdout);
        assert_eq!(listed["id"], id, "{method}");
        assert!(listed.get("error").is_none(), "{method}: {listed}");
        assert_eq!(
            listed["result"][key].as_array().map(Vec::len),
            Some(0),
            "{method}: {listed}"
        );
    }

    // `resources/read` stays method-not-found for every URI shape, including one that names a
    // real readable file. Serving it would build a second file-reading contract outside the
    // annotated tool surface, and one whose own `Partial` note would name continuation
    // parameters `resources/read` has no field to carry.
    let sentinel_body = "d0f4b2e7-sentinel-never-served";
    let sentinel = temp.path().join("sentinel.txt");
    write(&sentinel, format!("{sentinel_body}\n"));
    for (id, uri) in [
        (6, format!("file:///{}", normalized(&sentinel))),
        (7, sentinel.to_string_lossy().into_owned()),
        (8, "file:///Z:/definitely/missing.txt".to_string()),
    ] {
        send(
            &mut stdin,
            serde_json::json!({
                "jsonrpc":"2.0","id":id,"method":"resources/read","params":{"uri":&uri}
            }),
        );
        let rejected = read_response(&mut stdout);
        assert_eq!(rejected["id"], id, "{uri}");
        assert_eq!(rejected["error"]["code"], -32601, "{uri}: {rejected}");
        assert!(rejected.get("result").is_none(), "{uri}: {rejected}");
        assert!(
            !rejected.to_string().contains(sentinel_body),
            "{uri}: {rejected}"
        );
    }

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
#[cfg(feature = "pdf")]
// Repair only happens on a control center that has not already released the engine, so each half
// of this test needs a private one. `FASTCTX_TEST_BUILD_ID` is the only way to get that, and it is
// debug-only: in a release build both halves share one host and the second never re-extracts.
#[cfg(debug_assertions)]
fn stdio_pdf_call_repairs_a_corrupted_cached_engine() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("page.pdf");
    write_pdf(&pdf, &[Some("Cache repair")]);
    let cache_root = temp.path().join("cache-root");
    let process_id = std::process::id();

    let mut first_command = fastctx_command();
    first_command.env(
        "FASTCTX_TEST_BUILD_ID",
        format!("pdf-repair-a-{process_id}"),
    );
    let engine_dir = configure_isolated_cache(&mut first_command, &cache_root);
    let first = call_tool(
        first_command,
        "inspect_local_file",
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

    let mut second_command = fastctx_command();
    second_command.env(
        "FASTCTX_TEST_BUILD_ID",
        format!("pdf-repair-b-{process_id}"),
    );
    configure_isolated_cache(&mut second_command, &cache_root);
    let second = call_tool(
        second_command,
        "inspect_local_file",
        serde_json::json!({"file_path": normalized(&pdf)}),
    );
    assert_eq!(second["result"]["isError"], false);
    assert_eq!(std::fs::read(engine).unwrap(), original);
}

#[test]
#[cfg(all(feature = "pdf", any(windows, all(unix, not(target_os = "macos")))))]
fn stdio_pdf_initialization_uses_the_request_session_cache_environment() {
    let temp = tempfile::tempdir().unwrap();
    let text = temp.path().join("plain.txt");
    let pdf = temp.path().join("page.pdf");
    write(&text, b"plain\n");
    write_pdf(&pdf, &[Some("Session cache")]);
    let bootstrap_cache = temp.path().join("bootstrap-cache");
    let request_cache = temp.path().join("request-cache");
    let home = temp.path().join("home");

    // A shared private HOME selects one fresh control center in every build profile; unlike the
    // debug-only build-id hook, this also keeps release tests isolated from an earlier PDF user.
    let mut bootstrap = fastctx_command_for_home(&home);
    let bootstrap_engine = configure_isolated_cache(&mut bootstrap, &bootstrap_cache);
    let first = call_tool(
        bootstrap,
        "inspect_local_file",
        serde_json::json!({"file_path": normalized(&text)}),
    );
    assert_eq!(first["result"]["isError"], false);

    let mut request = fastctx_command_for_home(&home);
    let request_engine = configure_isolated_cache(&mut request, &request_cache);
    let second = call_tool(
        request,
        "inspect_local_file",
        serde_json::json!({"file_path": normalized(&pdf)}),
    );
    assert_eq!(second["result"]["isError"], false);
    assert!(
        std::fs::read_dir(&request_engine)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_file())
                    && !entry.file_name().to_string_lossy().ends_with(".lock")
            }),
        "PDF initialization did not release the engine into the request cache"
    );
    assert!(
        !bootstrap_engine.exists()
            || std::fs::read_dir(&bootstrap_engine)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .all(|entry| entry.file_name().to_string_lossy().ends_with(".lock")),
        "PDF initialization used the control center's bootstrap cache"
    );
}

#[test]
#[cfg(not(feature = "pdf"))]
fn no_pdf_build_rejects_pdf_without_affecting_the_public_read_schema() {
    let temp = tempfile::tempdir().unwrap();
    let pdf = temp.path().join("disabled.pdf");
    write(&pdf, b"%PDF-1.4\n");
    let response = call_tool(
        fastctx_command(),
        "inspect_local_file",
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
