mod common;

use common::{McpSession, glob_files, grep_files, mcp_text, normalized, text, write};
use fastctx::glob_tool::{FilterMode, GlobRequest, SortMode};
use fastctx::grep_tool::{GrepRequest, OutputMode};
use fastctx::read_tool::{BatchReadEntry, ReadRequest, read_file};
use std::process::Command;

#[test]
fn independent_o200k_oracle_keeps_high_entropy_outputs_below_the_budget() {
    const BUDGET: usize = 256;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("entropy.txt");
    let source_text = (1..=400).map(entropy_line).collect::<Vec<_>>().join("\n");
    write(&source, source_text.as_bytes());
    let batch_second = temp.path().join("entropy-second.txt");
    let batch_third = temp.path().join("entropy-third.txt");
    write(&batch_second, source_text.as_bytes());
    write(&batch_third, source_text.as_bytes());

    let glob_root = temp.path().join("glob");
    for index in 0..300 {
        write(
            &glob_root.join(format!(
                "batch-{batch:03}/item-{index:04}-{suffix}.txt",
                batch = index / 20,
                suffix = pseudo_hex(index as u64)
            )),
            b"fixture",
        );
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .args(["serve", "--enable-shell"])
        .env("FASTCTX_TOKEN_BUDGET", BUDGET.to_string())
        .env("FASTCTX_READ_TOKEN_BUDGET", BUDGET.to_string())
        .env("FASTCTX_GREP_TOKEN_BUDGET", BUDGET.to_string())
        .env("FASTCTX_GLOB_TOKEN_BUDGET", BUDGET.to_string())
        .env("FASTCTX_RUN_TOKEN_BUDGET", BUDGET.to_string());
    let mut session = McpSession::start(command);

    let batch_response = session.call(
        "read",
        serde_json::json!({
            "files": [
                {"path": normalized(&source), "limit": 50, "encoding": "utf-8"},
                {"path": normalized(&batch_second)},
                {"path": normalized(&batch_third)}
            ]
        }),
    );

    let cases = [
        session.call(
            "read",
            serde_json::json!({"file_path": normalized(&source), "limit": 2000}),
        ),
        session.call(
            "grep",
            serde_json::json!({
                "pattern": "HIT",
                "path": normalized(&source),
                "output_mode": "content",
                "head_limit": 0
            }),
        ),
        session.call(
            "glob",
            serde_json::json!({
                "pattern": "**/*.txt",
                "path": normalized(&glob_root),
                "filter_mode": "all",
                "limit": 1000
            }),
        ),
        session.call(
            "run",
            serde_json::json!({
                "command": "for ((i=0; i<400; i++)); do printf 'HIT !@#%%^&*()[]{} <> /\\ | emoji=😀🚀 combining=é %04d\\n' \"$i\"; done",
                "login_shell": false
            }),
        ),
        batch_response.clone(),
    ];

    for response in &cases {
        assert_eq!(response["result"]["isError"], false, "{response}");
        let output = mcp_text(response);
        let oracle_tokens = tiktoken_rs::o200k_base_singleton()
            .encode_ordinary(output)
            .len();
        assert!(
            oracle_tokens <= BUDGET,
            "independent oracle counted {oracle_tokens} tokens over budget {BUDGET}: {output}"
        );
        let terminal = output.lines().last().unwrap();
        assert!(terminal.starts_with("(Partial:"), "{terminal}");
        assert!(
            terminal.ends_with(".)"),
            "terminal note was cut: {terminal}"
        );
    }

    let batch_output = mcp_text(&batch_response);
    let (batch_body, batch_terminal) = split_terminal(batch_output);
    assert!(
        batch_body.starts_with(&format!("=== {} (lines 1-", normalized(&source))),
        "{batch_output}"
    );
    assert!(
        !batch_body.contains(&normalized(&batch_second)),
        "{batch_output}"
    );
    let pending_json = batch_terminal
        .strip_prefix("(Partial: 0 of 3 files processed. Continue with files=")
        .and_then(|terminal| terminal.strip_suffix(".)"))
        .unwrap_or_else(|| panic!("unexpected batch terminal: {batch_terminal}"));
    assert!(
        pending_json.starts_with(&format!(
            "[{{\"path\":\"{}\",\"offset\":",
            normalized(&source)
        )),
        "{pending_json}"
    );
    let pending: Vec<serde_json::Value> = serde_json::from_str(pending_json).unwrap();
    assert_eq!(pending.len(), 3);
    let next_offset = pending[0]["offset"].as_u64().unwrap() as usize;
    assert!(next_offset > 1);
    assert_eq!(pending[0]["limit"], 50 - (next_offset - 1));
    assert_eq!(pending[0]["encoding"], "utf-8");
    assert_eq!(pending[1]["path"], normalized(&batch_second));
    assert_eq!(pending[2]["path"], normalized(&batch_third));

    assert!(session.close().success());
}

#[test]
fn stdio_grep_and_glob_errors_obey_tiny_budgets_with_an_independent_oracle() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    write(&source, b"hit\n");
    let missing = temp.path().join("missing");
    let tokenizer = tiktoken_rs::o200k_base_singleton();

    for budget in [1_usize, 2] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
        command
            .arg("serve")
            .env("FASTCTX_TOKEN_BUDGET", budget.to_string())
            .env("FASTCTX_GREP_TOKEN_BUDGET", budget.to_string())
            .env("FASTCTX_GLOB_TOKEN_BUDGET", budget.to_string());
        let mut session = McpSession::start(command);

        let responses = [
            session.call(
                "grep",
                serde_json::json!({"pattern": "hit", "path": normalized(&missing)}),
            ),
            session.call(
                "grep",
                serde_json::json!({"pattern": "[", "path": normalized(&source)}),
            ),
            session.call(
                "grep",
                serde_json::json!({
                    "pattern": "hit",
                    "path": normalized(&source),
                    "encoding": "not-a-real-encoding"
                }),
            ),
            session.call(
                "grep",
                serde_json::json!({
                    "pattern": "hit",
                    "path": normalized(&source),
                    "output_mode": "content"
                }),
            ),
            session.call(
                "glob",
                serde_json::json!({"pattern": "**/*", "path": normalized(&missing)}),
            ),
            session.call(
                "glob",
                serde_json::json!({"pattern": "[", "path": normalized(temp.path())}),
            ),
            session.call(
                "glob",
                serde_json::json!({
                    "pattern": "**/*.txt",
                    "path": normalized(temp.path()),
                    "filter_mode": "all"
                }),
            ),
        ];

        for response in responses {
            assert_eq!(response["result"]["isError"], true, "{response}");
            let output = mcp_text(&response);
            let tokens = tokenizer.encode_ordinary(output).len();
            assert!(
                tokens <= budget,
                "independent oracle counted {tokens} tokens over budget {budget}: {output:?}"
            );
        }
        assert!(session.close().success());
    }
}

#[test]
fn read_offset_pages_reassemble_without_duplicates_or_gaps_across_a_matrix() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("read-pages.txt");

    for total in [1_usize, 2, 3, 7, 31, 64] {
        let expected = (1..=total)
            .map(|line| format!("value-{line:03}"))
            .collect::<Vec<_>>();
        write(&path, expected.join("\n").as_bytes());

        for page_width in [1_usize, 2, 3, 7, 16] {
            let mut offset = 1_usize;
            let mut actual = Vec::new();
            while offset <= total {
                let output = text(read_file(ReadRequest {
                    file_path: Some(normalized(&path)),
                    files: None,
                    offset: Some(offset),
                    limit: Some(page_width),
                    pages: None,
                    pdf_mode: None,
                    encoding: None,
                    view: None,
                }));
                let (body, terminal) = split_terminal(&output);
                let page = body
                    .lines()
                    .map(|line| {
                        let (number, value) = line.split_once('\t').unwrap();
                        (number.parse::<usize>().unwrap(), value.to_string())
                    })
                    .collect::<Vec<_>>();
                assert!(!page.is_empty(), "{output}");
                for (expected_number, (number, value)) in
                    (offset..).zip(page.iter()).take(page.len())
                {
                    assert_eq!(*number, expected_number, "{output}");
                    assert_eq!(value, &expected[expected_number - 1], "{output}");
                    actual.push(value.clone());
                }
                offset += page.len();
                assert_terminal_matches_more(terminal, offset <= total);
            }
            assert_eq!(actual, expected, "total={total}, page_width={page_width}");
        }
    }
}

#[test]
fn batch_read_continuation_chain_matches_independent_per_file_line_oracles() {
    let temp = tempfile::tempdir().unwrap();
    let fixtures = [3_usize, 7, 31]
        .into_iter()
        .enumerate()
        .map(|(file_index, total)| {
            let path = temp.path().join(format!("batch-{file_index}.txt"));
            let expected = (1..=total)
                .map(|line| format!("file-{file_index}-value-{line:03}"))
                .collect::<Vec<_>>();
            write(&path, expected.join("\n").as_bytes());
            (path, expected)
        })
        .collect::<Vec<_>>();

    for page_width in [1_usize, 2, 5, 11] {
        let mut pending = fixtures
            .iter()
            .map(|(path, _)| BatchReadEntry {
                path: normalized(path),
                offset: None,
                limit: Some(page_width),
                encoding: None,
            })
            .collect::<Vec<_>>();
        let mut actual = std::collections::BTreeMap::<String, Vec<(usize, String)>>::new();
        let mut calls = 0;
        loop {
            calls += 1;
            assert!(calls <= 3, "continuation did not converge");
            let requested_paths = pending
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>();
            let output = text(read_file(ReadRequest {
                file_path: None,
                files: Some(pending),
                offset: None,
                limit: None,
                pages: None,
                pdf_mode: None,
                encoding: None,
                view: None,
            }));
            let (body, terminal) = split_terminal(&output);
            let mut current_path = None;
            let mut delivered_paths = Vec::new();
            for line in body.lines() {
                if let Some(header) = line.strip_prefix("=== ") {
                    let path = header
                        .split_once(" (lines ")
                        .map(|(path, _)| path)
                        .unwrap_or_else(|| panic!("invalid batch header: {line}"));
                    current_path = Some(path.to_string());
                    delivered_paths.push(path.to_string());
                } else if let Some((number, value)) = line.split_once('\t') {
                    let path = current_path
                        .as_ref()
                        .unwrap_or_else(|| panic!("line before batch header: {line}"));
                    actual
                        .entry(path.clone())
                        .or_default()
                        .push((number.parse().unwrap(), value.to_string()));
                }
            }
            assert_eq!(
                delivered_paths,
                requested_paths[..delivered_paths.len()],
                "request order changed: {output}"
            );

            if terminal.starts_with("(Complete:") {
                break;
            }
            let json = terminal
                .split_once("Continue with files=")
                .and_then(|(_, tail)| tail.strip_suffix(".)"))
                .unwrap_or_else(|| panic!("invalid batch continuation: {terminal}"));
            pending = serde_json::from_str(json).unwrap();
        }

        for (path, expected) in &fixtures {
            let delivered = actual.remove(&normalized(path)).unwrap();
            assert_eq!(
                delivered.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
                (1..=expected.len()).collect::<Vec<_>>()
            );
            assert_eq!(
                delivered
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>(),
                *expected
            );
        }
        assert!(actual.is_empty());
    }
}

#[test]
fn grep_occurrence_pages_reassemble_without_duplicates_or_gaps_across_a_matrix() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("grep-pages.txt");
    let mut lines = Vec::new();
    let mut expected = Vec::new();
    for line_number in 1..=48 {
        let occurrences = 1 + (next_state(line_number as u64) as usize % 4);
        lines.push(
            std::iter::repeat_n("HIT", occurrences)
                .collect::<Vec<_>>()
                .join(" separator "),
        );
        expected.extend(std::iter::repeat_n(
            format!("{line_number}:HIT"),
            occurrences,
        ));
    }
    write(&path, lines.join("\n").as_bytes());

    for page_width in [1_usize, 2, 5, 11, 29] {
        let mut offset = 0_usize;
        let mut actual = Vec::new();
        while offset < expected.len() {
            let output = text(grep_files(GrepRequest {
                pattern: "HIT".to_string(),
                path: Some(normalized(&path)),
                glob: None,
                file_type: None,
                output_mode: Some(OutputMode::Content),
                case_insensitive: None,
                line_numbers: Some(true),
                only_matching: Some(true),
                before_context: None,
                after_context: None,
                context: None,
                multiline: None,
                head_limit: Some(page_width),
                offset: Some(offset),
                encoding: None,
                fallback_encoding: None,
            }));
            let (body, terminal) = split_terminal(&output);
            let page = body.lines().map(str::to_string).collect::<Vec<_>>();
            assert!(!page.is_empty(), "{output}");
            actual.extend(page.iter().cloned());
            offset += page.len();
            assert_terminal_matches_more(terminal, offset < expected.len());
        }
        assert_eq!(actual, expected, "page_width={page_width}");
    }
}

#[test]
fn glob_offset_pages_reassemble_without_duplicates_or_gaps_across_a_matrix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("glob-pages");
    let mut expected = Vec::new();
    for index in 0..73 {
        let path = root.join(format!(
            "group-{group:02}/item-{suffix}-{index:03}.dat",
            group = (index * 17) % 11,
            suffix = pseudo_hex(next_state(index as u64))
        ));
        write(&path, b"fixture");
        expected.push(normalized(&path));
    }
    expected.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));

    for page_width in [1_usize, 2, 7, 13, 31] {
        let mut offset = 0_usize;
        let mut actual = Vec::new();
        while offset < expected.len() {
            let output = text(glob_files(GlobRequest {
                pattern: "**/*.dat".to_string(),
                path: Some(normalized(&root)),
                filter_mode: Some(FilterMode::All),
                sort: Some(SortMode::Path),
                offset: Some(offset),
                limit: Some(page_width),
            }));
            let (body, terminal) = split_terminal(&output);
            let page = body.lines().map(str::to_string).collect::<Vec<_>>();
            assert!(!page.is_empty(), "{output}");
            actual.extend(page.iter().cloned());
            offset += page.len();
            assert_terminal_matches_more(terminal, offset < expected.len());
        }
        assert_eq!(actual, expected, "page_width={page_width}");
    }
}

fn split_terminal(output: &str) -> (&str, &str) {
    output
        .rsplit_once("\n\n")
        .unwrap_or_else(|| panic!("response has no terminal separator: {output}"))
}

fn assert_terminal_matches_more(terminal: &str, has_more: bool) {
    assert_eq!(terminal.starts_with("(Partial:"), has_more, "{terminal}");
    assert_eq!(terminal.starts_with("(Complete:"), !has_more, "{terminal}");
    assert!(
        terminal.ends_with(".)"),
        "terminal note was cut: {terminal}"
    );
}

fn entropy_line(index: usize) -> String {
    format!(
        "HIT {index:04} !@#$%^&*()[]{{}} <> /\\ | base64={} emoji=😀🚀 combining=é hash={}",
        "QWxhZGRpbjpvcGVuIHNlc2FtZQ==".repeat(3),
        pseudo_hex(index as u64)
    )
}

fn pseudo_hex(seed: u64) -> String {
    format!(
        "{:016x}{:016x}",
        next_state(seed),
        next_state(seed ^ u64::MAX)
    )
}

fn next_state(mut state: u64) -> u64 {
    state = state.wrapping_mul(6_364_136_223_846_793_005);
    state.wrapping_add(1_442_695_040_888_963_407)
}
