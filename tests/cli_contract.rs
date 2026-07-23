mod common;

use common::{McpSession, mcp_text, normalized};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[test]
fn help_version_ui_guard_and_language_errors_are_scriptable() {
    let version = command().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("fastctx {}\n", env!("CARGO_PKG_VERSION"))
    );

    let help = command().arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for subcommand in ["serve", "ui", "apply", "unapply", "status", "lang", "jobs"] {
        assert!(help.contains(subcommand), "{help}");
    }
    assert!(!help.contains("shell-serve"), "{help}");
    assert!(!help.contains("edit-serve"), "{help}");
    assert!(!help.contains("job-host"), "{help}");
    assert!(!help.contains("job-bootstrap"), "{help}");
    assert!(!help.contains("job-watch"), "{help}");
    let serve_help = command().args(["serve", "--help"]).output().unwrap();
    let serve_help = String::from_utf8(serve_help.stdout).unwrap();
    assert!(serve_help.contains("--enable-shell"), "{serve_help}");
    assert!(!serve_help.contains("--enable-edit"), "{serve_help}");
    let jobs_help = command().args(["jobs", "--help"]).output().unwrap();
    assert!(jobs_help.status.success());
    let jobs_help = String::from_utf8(jobs_help.stdout).unwrap();
    assert!(jobs_help.contains("kill"), "{jobs_help}");
    for subcommand in ["apply", "unapply", "status", "doctor"] {
        let output = command().args([subcommand, "--help"]).output().unwrap();
        assert!(output.status.success(), "{subcommand}");
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("--codex-home"), "{subcommand}: {output}");
    }

    let ui = command().arg("ui").output().unwrap();
    assert!(!ui.status.success());
    assert!(String::from_utf8_lossy(&ui.stderr).contains("requires both stdin and stdout"));

    let temp = tempfile::tempdir().unwrap();
    let invalid = isolated_command(temp.path())
        .args(["lang", "not-a-language"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let error = String::from_utf8_lossy(&invalid.stderr);
    for code in ["en", "zh-CN", "ja", "ru", "vi", "uk"] {
        assert!(error.contains(code), "{error}");
    }

    for code in [
        "en", "zh-CN", "zh-TW", "ja", "ko", "es", "fr", "de", "pt-BR", "ru", "it", "tr", "pl",
        "nl", "vi", "id", "uk",
    ] {
        let valid = isolated_command(temp.path())
            .args(["lang", code])
            .output()
            .unwrap();
        assert!(
            valid.status.success(),
            "{}: {}",
            code,
            String::from_utf8_lossy(&valid.stderr)
        );
        let settings = std::fs::read_to_string(temp.path().join(".fastctx/config.toml")).unwrap();
        let parsed: fastctx::control::settings::FastCtxSettings =
            toml_edit::de::from_str(&settings).unwrap();
        assert_eq!(parsed.language.as_deref(), Some(code), "{settings}");
    }
}

#[test]
fn default_pipe_and_serve_flags_route_exact_tool_sets() {
    for (arguments, expected) in [
        (vec![], vec!["glob", "grep", "read", "replace"]),
        (vec!["serve"], vec!["glob", "grep", "read", "replace"]),
        (
            vec!["serve", "--enable-shell"],
            vec![
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
            vec!["serve", "--enable-edit"],
            vec!["glob", "grep", "read", "replace"],
        ),
        (
            vec!["serve", "--enable-shell", "--enable-edit"],
            vec![
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
    ] {
        let mut command = command();
        command.args(&arguments);
        let mut session = McpSession::start(command);
        assert_eq!(session.list_tools(), expected, "{arguments:?}");
        assert!(session.close().success());
    }
}

#[test]
fn jobs_commands_have_scriptable_empty_and_failure_paths() {
    let temp = tempfile::tempdir().unwrap();

    let empty = isolated_command(temp.path()).arg("jobs").output().unwrap();
    assert!(empty.status.success());
    assert_eq!(
        String::from_utf8(empty.stdout).unwrap(),
        "No running jobs.\n"
    );
    assert!(empty.stderr.is_empty());

    let missing = isolated_command(temp.path())
        .args(["jobs", "kill", "j-000001"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert_eq!(
        String::from_utf8(missing.stderr).unwrap(),
        "fastctx: No such job: \"j-000001\". It may never have existed, or its finished record was evicted by the job storage limit. List known jobs with job_list.\n"
    );
}

#[test]
fn jobs_kill_manages_a_persistent_job_and_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    write_shell_settings(temp.path(), true);
    let job_id = start_persistent_job(temp.path(), "sleep 60");
    let mut cleanup = BackgroundJobCleanup::new(temp.path(), &job_id);

    let listed = isolated_command(temp.path()).arg("jobs").output().unwrap();
    assert_success(&listed);
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.contains(&job_id), "{listed}");
    assert!(listed.contains("sleep 60"), "{listed}");

    let killed = isolated_command(temp.path())
        .args(["jobs", "kill", &job_id])
        .output()
        .unwrap();
    assert_success(&killed);
    assert_eq!(
        String::from_utf8(killed.stdout).unwrap(),
        format!("Job {job_id} killed.\n")
    );
    let idempotent = isolated_command(temp.path())
        .args(["jobs", "kill", &job_id])
        .output()
        .unwrap();
    assert_success(&idempotent);
    let idempotent = String::from_utf8(idempotent.stdout).unwrap();
    assert!(
        idempotent.starts_with(&format!("Job {job_id} had already exited with code ")),
        "{idempotent}"
    );
    let empty = isolated_command(temp.path()).arg("jobs").output().unwrap();
    assert_success(&empty);
    assert_eq!(
        String::from_utf8(empty.stdout).unwrap(),
        "No running jobs.\n"
    );
    cleanup.disarm();
}

#[test]
fn unapply_stops_a_real_persistent_job_before_removing_fastctx_data() {
    let temp = tempfile::tempdir().unwrap();
    write_shell_settings(temp.path(), true);
    let applied = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .output()
        .unwrap();
    assert_success(&applied);

    let job_id = start_persistent_job(temp.path(), "sleep 60");
    let mut cleanup = BackgroundJobCleanup::new(temp.path(), &job_id);

    let running = isolated_command(temp.path()).arg("jobs").output().unwrap();
    assert_success(&running);
    let running = String::from_utf8(running.stdout).unwrap();
    assert!(running.contains(&job_id), "{running}");
    assert!(running.contains("sleep 60"), "{running}");

    let removed = isolated_command(temp.path())
        .args(["unapply", "--yes"])
        .output()
        .unwrap();
    assert_success(&removed);
    let output = String::from_utf8(removed.stdout).unwrap();
    assert!(
        output.contains("Stop      1 running background job before removal"),
        "{output}"
    );
    assert!(
        output.contains("Stopped 1 running background job before removal."),
        "{output}"
    );
    assert!(!temp.path().join(".fastctx").exists());
    cleanup.disarm();
}

#[test]
fn explicit_serve_performs_a_real_initialize_and_tools_list() {
    let mut child = command()
        .arg("serve")
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
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-06-18",
                "capabilities":{},
                "clientInfo":{"name":"cli-contract","version":"1.0"}
            }
        }),
    );
    assert_eq!(read_response(&mut stdout)["id"], 1);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let response = read_response(&mut stdout);
    let names = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["glob", "grep", "read", "replace"]);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn serve_rejects_an_invalid_search_cpu_limit_before_stdio_and_preserves_source_bytes() {
    let temp = profile_test_home();
    let settings_dir = temp.path().join(".fastctx");
    let settings_path = settings_dir.join("config.toml");
    std::fs::create_dir_all(&settings_dir).unwrap();
    let original = b"schema_version = 1\n\n[search]\nmax_cpu_cores = 0\n";
    std::fs::write(&settings_path, original).unwrap();

    let output = isolated_command(temp.path()).arg("serve").output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    let maximum = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 16);
    for expected in [
        normalized(&settings_path),
        "search.max_cpu_cores".to_string(),
        format!("1..={maximum}"),
        "Repair the value and retry.".to_string(),
    ] {
        assert!(error.contains(&expected), "missing {expected:?}:\n{error}");
    }
    assert_eq!(std::fs::read(settings_path).unwrap(), original);
}

#[test]
fn apply_preserves_search_cpu_settings_without_copying_them_into_codex_env() {
    let temp = tempfile::tempdir().unwrap();
    let settings_dir = temp.path().join(".fastctx");
    std::fs::create_dir_all(&settings_dir).unwrap();
    std::fs::write(
        settings_dir.join("config.toml"),
        b"schema_version = 1\n\n[search]\nmax_cpu_cores = 1\n",
    )
    .unwrap();

    let output = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .output()
        .unwrap();
    assert_success(&output);

    let settings =
        fastctx::control::settings::load_from(&settings_dir.join("config.toml")).unwrap();
    assert_eq!(settings.search.max_cpu_cores, Some(1));
    let codex_source = std::fs::read_to_string(temp.path().join(".codex/config.toml")).unwrap();
    assert!(!codex_source.contains("max_cpu_cores"), "{codex_source}");
    assert!(!codex_source.contains("SEARCH_CPU"), "{codex_source}");
    let document = codex_source.parse::<toml_edit::DocumentMut>().unwrap();
    let env = document["mcp_servers"]["fastctx"]["env"]
        .as_table_like()
        .unwrap();
    assert!(!env.is_empty());
    assert!(
        env.iter()
            .all(|(key, _)| key.starts_with("FASTCTX_") && key.ends_with("_TOKEN_BUDGET")),
        "{codex_source}"
    );
}

#[test]
fn configured_search_parallelism_keeps_grep_and_glob_bytes_equal_at_one_middle_and_maximum() {
    let temp = profile_test_home();
    let workspace = temp.path().join("search-fixture");
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let first = workspace.join("a.txt");
    let second = nested.join("b.txt");
    std::fs::write(&first, b"before\nneedle one\nafter\n").unwrap();
    std::fs::write(&second, b"no match here\n").unwrap();
    std::fs::write(workspace.join("ignored.log"), b"ordinary log\n").unwrap();
    for index in 0..32 {
        std::fs::write(
            workspace.join(format!("candidate-{index:02}.dat")),
            b"ordinary candidate\n",
        )
        .unwrap();
    }
    let expected_grep_files = format!("{}\n\n(Complete: all 1 file shown.)", normalized(&first));
    let expected_glob = format!(
        "{}\n{}\n\n(Complete: all 2 files shown.)",
        normalized(&first),
        normalized(&second)
    );
    let expected_content = format!(
        "{}\n2:needle one\n\n(Complete: all 1 result shown.)",
        normalized(&first)
    );
    let maximum = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 16);
    let parallelism = std::collections::BTreeSet::from([1, (maximum / 2).max(1), maximum]);

    for configured in parallelism {
        let settings_dir = temp.path().join(".fastctx");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(
            settings_dir.join("config.toml"),
            format!("schema_version = 1\n\n[search]\nmax_cpu_cores = {configured}\n"),
        )
        .unwrap();
        let mut server = isolated_command(temp.path());
        server.arg("serve");
        let mut session = McpSession::start(server);
        let grep_files = session.call(
            "grep",
            serde_json::json!({
                "pattern": "needle",
                "path": normalized(&workspace),
                "glob": "**/*",
                "output_mode": "files_with_matches",
                "head_limit": 100,
                "offset": 0
            }),
        );
        assert_eq!(mcp_text(&grep_files), expected_grep_files, "P={configured}");
        let grep_content = session.call(
            "grep",
            serde_json::json!({
                "pattern": "needle",
                "path": normalized(&workspace),
                "glob": "**/*",
                "output_mode": "content",
                "line_numbers": true,
                "head_limit": 100,
                "offset": 0
            }),
        );
        assert_eq!(mcp_text(&grep_content), expected_content, "P={configured}");
        let glob = session.call(
            "glob",
            serde_json::json!({
                "pattern": "**/*.txt",
                "path": normalized(&workspace),
                "filter_mode": "all",
                "sort": "path",
                "limit": 100,
                "offset": 0
            }),
        );
        assert_eq!(mcp_text(&glob), expected_glob, "P={configured}");
        assert!(session.close().success(), "P={configured}");
    }
}

#[test]
fn noninteractive_apply_is_idempotent_and_unapply_restores_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    let config = concat!(
        "# user config\n",
        "tool_output_token_limit = 9000 # user value\n",
        "\n",
        "[mcp_servers.other]\n",
        "command = 'other'\n",
        "\n",
        "[features.code_mode]\n",
        "direct_only_tool_namespaces = [ 'other' ]\n",
    );
    let agents = "# User rules\n\nLeave this byte-for-byte.\n";
    std::fs::write(codex.join("config.toml"), config).unwrap();
    std::fs::write(codex.join("AGENTS.md"), agents).unwrap();

    let first = isolated_command(temp.path())
        .args(["apply", "--tier", "high", "--yes"])
        .output()
        .unwrap();
    assert_success(&first);
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    assert!(first_stdout.contains("Shared") || first_stdout.contains("Changed"));
    assert!(
        temp.path()
            .join(if cfg!(windows) {
                ".fastctx/bin/fastctx.exe"
            } else {
                ".fastctx/bin/fastctx"
            })
            .is_file()
    );
    let applied = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    assert!(applied.contains("[mcp_servers.fastctx]"), "{applied}");
    assert!(applied.contains("mcp__fastctx"), "{applied}");
    assert!(
        applied.contains("tool_output_token_limit = 16000 # user value"),
        "{applied}"
    );
    let managed_paths = [
        codex.join("config.toml"),
        codex.join("AGENTS.md"),
        temp.path().join(if cfg!(windows) {
            ".fastctx/bin/fastctx.exe"
        } else {
            ".fastctx/bin/fastctx"
        }),
        temp.path().join(".fastctx/config.toml"),
    ];
    let first_bytes = managed_paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let second = isolated_command(temp.path())
        .args(["apply", "--tier", "high", "--yes"])
        .output()
        .unwrap();
    assert_success(&second);
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(second_stdout.contains("No changes") || second_stdout.contains("Changed 0"));
    for (path, expected) in managed_paths.iter().zip(first_bytes) {
        assert_eq!(std::fs::read(path).unwrap(), expected, "{}", path.display());
    }
    // File backups were removed completely; no path may emit a .fastctx-backup file (2026-07-12 regression).
    assert!(
        backup_files(temp.path()).is_empty(),
        "no .fastctx-backup files should ever be written"
    );

    let empty_path = temp.path().join("empty-path");
    std::fs::create_dir_all(&empty_path).unwrap();
    let mut status_command = isolated_command(temp.path());
    status_command.arg("status").env("PATH", &empty_path);
    let status = status_command.output().unwrap();
    assert_success(&status);
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_text.contains("[PASS] Codex profile"),
        "{status_text}"
    );
    assert!(
        !status_text.contains("ChatGPT / Codex host") && !status_text.contains("codex-cli"),
        "{status_text}"
    );
    assert!(!status_text.contains("[FAIL]"), "{status_text}");
    assert!(
        status_text.contains("[PASS] Installed binary"),
        "{status_text}"
    );

    let applied_bytes = std::fs::read(codex.join("config.toml")).unwrap();
    let mut host_rewritten = applied_bytes.clone();
    host_rewritten
        .extend_from_slice(b"\n[plugins.runtime]\nlast_refresh = \"2026-07-17T00:01:00Z\"\n");
    std::fs::write(codex.join("config.toml"), &host_rewritten).unwrap();
    let mut host_status_command = isolated_command(temp.path());
    host_status_command.arg("status").env("PATH", &empty_path);
    let host_status = host_status_command.output().unwrap();
    assert_success(&host_status);
    let host_status_text = String::from_utf8_lossy(&host_status.stdout);
    assert!(
        host_status_text.contains("[PASS] Applied state"),
        "{host_status_text}"
    );
    let host_reapply = isolated_command(temp.path())
        .args(["apply", "--tier", "high", "--yes"])
        .output()
        .unwrap();
    assert_success(&host_reapply);
    let host_reapply_text = String::from_utf8_lossy(&host_reapply.stdout);
    assert!(
        host_reapply_text.contains("No changes") || host_reapply_text.contains("Changed 0"),
        "{host_reapply_text}"
    );
    assert_eq!(
        std::fs::read(codex.join("config.toml")).unwrap(),
        host_rewritten
    );

    let applied_agents = std::fs::read(codex.join("AGENTS.md")).unwrap();
    let mut user_extended_agents = applied_agents.clone();
    user_extended_agents.extend_from_slice(b"\nUser rule added after Apply.\n");
    std::fs::write(codex.join("AGENTS.md"), &user_extended_agents).unwrap();
    let mut agents_status_command = isolated_command(temp.path());
    agents_status_command.arg("status").env("PATH", &empty_path);
    let agents_status = agents_status_command.output().unwrap();
    assert_success(&agents_status);
    let agents_status_text = String::from_utf8_lossy(&agents_status.stdout);
    assert!(
        agents_status_text.contains("[PASS] AGENTS guidance"),
        "{agents_status_text}"
    );

    let drifted_agents = String::from_utf8(user_extended_agents)
        .unwrap()
        .replace("mcp__fastctx__read", "mcp__fastctx__read_broken");
    std::fs::write(codex.join("AGENTS.md"), drifted_agents).unwrap();
    let mut agents_drift_command = isolated_command(temp.path());
    agents_drift_command.arg("status").env("PATH", &empty_path);
    let agents_drift = agents_drift_command.output().unwrap();
    assert_eq!(agents_drift.status.code(), Some(1));
    let agents_drift_text = String::from_utf8_lossy(&agents_drift.stdout);
    assert!(
        agents_drift_text.contains("[FAIL] AGENTS guidance"),
        "{agents_drift_text}"
    );
    std::fs::write(codex.join("AGENTS.md"), applied_agents).unwrap();

    let drifted = String::from_utf8(host_rewritten).unwrap().replace(
        "tool_output_token_limit = 16000",
        "tool_output_token_limit = 15000",
    );
    std::fs::write(codex.join("config.toml"), drifted).unwrap();
    let mut drift_command = isolated_command(temp.path());
    drift_command.arg("status").env("PATH", &empty_path);
    let drift = drift_command.output().unwrap();
    assert_eq!(drift.status.code(), Some(1));
    let drift_text = String::from_utf8_lossy(&drift.stdout);
    assert!(drift_text.contains("[FAIL] Applied state"), "{drift_text}");
    assert!(
        drift_text.contains("tool_output_token_limit"),
        "{drift_text}"
    );
    assert!(drift_text.contains("Next:"), "{drift_text}");
    std::fs::write(codex.join("config.toml"), applied_bytes).unwrap();
    assert!(
        status_text.contains("[PASS] MCP handshake"),
        "{status_text}"
    );

    let removed = isolated_command(temp.path())
        .args(["unapply", "--yes"])
        .output()
        .unwrap();
    assert_success(&removed);
    assert_eq!(
        std::fs::read(codex.join("config.toml")).unwrap(),
        config.as_bytes()
    );
    assert_eq!(
        std::fs::read(codex.join("AGENTS.md")).unwrap(),
        agents.as_bytes()
    );
}

#[test]
fn noninteractive_apply_bootstraps_a_fresh_home_without_codex_cli_or_profile() {
    let temp = tempfile::tempdir().unwrap();
    let empty_path = temp.path().join("empty-path");
    std::fs::create_dir_all(&empty_path).unwrap();

    let mut initial_status = isolated_command(temp.path());
    initial_status.arg("status").env("PATH", &empty_path);
    let initial_status = initial_status.output().unwrap();
    assert_success(&initial_status);
    assert_eq!(initial_status.status.code(), Some(0));
    let initial_status_text = String::from_utf8_lossy(&initial_status.stdout);
    assert!(
        initial_status_text.contains("[INFO] Codex profile"),
        "{initial_status_text}"
    );
    assert!(!initial_status_text.contains("codex-cli"));

    let mut apply = isolated_command(temp.path());
    apply
        .args(["apply", "--tier", "standard", "--yes"])
        .env("PATH", &empty_path);
    let applied = apply.output().unwrap();
    assert_success(&applied);

    let codex = temp.path().join(".codex");
    assert!(codex.join("config.toml").is_file());
    assert!(codex.join("AGENTS.md").is_file());
    let config = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    assert!(config.contains("[mcp_servers.fastctx]"), "{config}");
    assert!(config.contains("FASTCTX_TOKEN_BUDGET = \"8500\""));

    let mut status = isolated_command(temp.path());
    status.arg("status").env("PATH", &empty_path);
    let status = status.output().unwrap();
    assert_success(&status);
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(!status_text.contains("[FAIL]"), "{status_text}");
    assert!(!status_text.contains("codex-cli"), "{status_text}");

    let mut unapply = isolated_command(temp.path());
    unapply.args(["unapply", "--yes"]).env("PATH", &empty_path);
    let removed = unapply.output().unwrap();
    assert_success(&removed);
    assert!(!codex.exists());
    assert!(!temp.path().join(".fastctx").exists());
}

#[test]
fn codex_home_env_selects_the_profile_without_moving_fastctx_state() {
    let temp = profile_test_home();
    let profile = temp.path().join("relocated-codex-profile");

    let applied = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .env("CODEX_HOME", &profile)
        .output()
        .unwrap();
    assert_success(&applied);
    assert!(profile.join("config.toml").is_file());
    assert!(profile.join("AGENTS.md").is_file());
    assert!(!profile.join(".codex").exists());
    assert!(!temp.path().join(".codex").exists());
    assert!(temp.path().join(".fastctx/config.toml").is_file());

    let status = isolated_command(temp.path())
        .arg("status")
        .env("CODEX_HOME", &profile)
        .output()
        .unwrap();
    assert_success(&status);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("[PASS] Codex profile"), "{status}");
    assert!(status.contains(&normalized(&profile)), "{status}");
    assert!(status.contains("source: env"), "{status}");

    let removed = isolated_command(temp.path())
        .args(["unapply", "--yes"])
        .env("CODEX_HOME", &profile)
        .output()
        .unwrap();
    assert_success(&removed);
    assert!(!profile.exists());
    assert!(!temp.path().join(".fastctx").exists());
}

#[test]
fn codex_home_flag_overrides_the_live_environment_for_all_control_commands() {
    let temp = profile_test_home();
    let environment_profile = temp.path().join("environment-profile");
    let flag_profile = temp.path().join("flag-profile");

    let applied = isolated_command(temp.path())
        .arg("apply")
        .arg("--codex-home")
        .arg(&flag_profile)
        .arg("--yes")
        .env("CODEX_HOME", &environment_profile)
        .output()
        .unwrap();
    assert_success(&applied);
    assert!(flag_profile.join("config.toml").is_file());
    assert!(!environment_profile.exists());

    for subcommand in ["status", "doctor"] {
        let status = isolated_command(temp.path())
            .arg(subcommand)
            .arg("--codex-home")
            .arg(&flag_profile)
            .env("CODEX_HOME", &environment_profile)
            .output()
            .unwrap();
        assert_success(&status);
        let status = String::from_utf8(status.stdout).unwrap();
        assert!(status.contains(&normalized(&flag_profile)), "{status}");
        assert!(status.contains("source: flag"), "{status}");
        assert!(
            !status.contains(&normalized(&environment_profile)),
            "{status}"
        );
    }

    let switched_status = isolated_command(temp.path())
        .arg("status")
        .env("CODEX_HOME", &environment_profile)
        .output()
        .unwrap();
    assert_success(&switched_status);
    let switched_status = String::from_utf8(switched_status.stdout).unwrap();
    assert!(switched_status.contains("source: env"), "{switched_status}");
    assert!(
        switched_status.contains("[INFO] Applied state"),
        "{switched_status}"
    );
    assert!(
        switched_status.contains("saved Apply receipt targets"),
        "{switched_status}"
    );

    let mismatched_apply = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .env("CODEX_HOME", &environment_profile)
        .output()
        .unwrap();
    assert!(!mismatched_apply.status.success());
    let mismatch = String::from_utf8(mismatched_apply.stderr).unwrap();
    assert!(
        mismatch.contains("does not match the last Apply receipt"),
        "{mismatch}"
    );
    assert!(mismatch.contains("source: env"), "{mismatch}");
    assert!(!environment_profile.exists());

    let mismatched_unapply = isolated_command(temp.path())
        .args(["unapply", "--yes"])
        .env("CODEX_HOME", &environment_profile)
        .output()
        .unwrap();
    assert!(!mismatched_unapply.status.success());
    let mismatch = String::from_utf8(mismatched_unapply.stderr).unwrap();
    assert!(
        mismatch.contains("does not match the last Apply receipt"),
        "{mismatch}"
    );
    assert!(mismatch.contains("source: env"), "{mismatch}");
    assert!(flag_profile.join("config.toml").is_file());
    assert!(temp.path().join(".fastctx/config.toml").is_file());

    let removed = isolated_command(temp.path())
        .arg("unapply")
        .arg("--codex-home")
        .arg(&flag_profile)
        .arg("--yes")
        .env("CODEX_HOME", &environment_profile)
        .output()
        .unwrap();
    assert_success(&removed);
    assert!(!flag_profile.exists());
    assert!(!environment_profile.exists());
}

#[test]
fn default_profile_source_is_visible_when_no_override_exists() {
    let temp = profile_test_home();
    let status = isolated_command(temp.path())
        .arg("status")
        .output()
        .unwrap();
    assert_success(&status);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(
        status.contains(&normalized(&temp.path().join(".codex"))),
        "{status}"
    );
    assert!(status.contains("source: default"), "{status}");
}

#[test]
fn codex_home_non_directory_is_reported_at_the_selected_target_without_writes() {
    let temp = profile_test_home();
    let profile = temp.path().join("profile-is-a-file");
    std::fs::write(&profile, b"user-owned").unwrap();

    let status = isolated_command(temp.path())
        .arg("doctor")
        .arg("--codex-home")
        .arg(&profile)
        .output()
        .unwrap();
    assert_eq!(status.status.code(), Some(1));
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("[FAIL] Codex profile"), "{status}");
    assert!(status.contains(&normalized(&profile)), "{status}");
    assert!(status.contains("source: flag"), "{status}");

    let apply = isolated_command(temp.path())
        .arg("apply")
        .arg("--codex-home")
        .arg(&profile)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!apply.status.success());
    let error = String::from_utf8(apply.stderr).unwrap();
    assert!(error.contains("is not a directory"), "{error}");
    assert!(error.contains(&normalized(&profile)), "{error}");
    assert_eq!(std::fs::read(&profile).unwrap(), b"user-owned");
    assert!(!temp.path().join(".fastctx").exists());
    assert!(!temp.path().join(".codex").exists());
}

#[test]
fn apply_status_and_unapply_cover_both_shell_states() {
    for fastshell in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        write_shell_settings(temp.path(), fastshell);
        let applied = isolated_command(temp.path())
            .args(["apply", "--yes"])
            .output()
            .unwrap();
        assert_success(&applied);
        let codex = temp.path().join(".codex");
        let config = std::fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(config.contains("[mcp_servers.fastctx]"), "{config}");
        let document = config.parse::<toml_edit::DocumentMut>().unwrap();
        let args = document["mcp_servers"]["fastctx"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        let mut expected_args = vec!["serve"];
        if fastshell {
            expected_args.push("--enable-shell");
        }
        assert_eq!(args, expected_args, "{config}");
        assert!(config.contains("mcp__fastctx"));
        assert_eq!(config.matches("mcp__fastctx").count(), 1);
        assert!(!config.contains("approval_mode"), "{config}");

        let agents = std::fs::read_to_string(codex.join("AGENTS.md")).unwrap();
        assert_eq!(agents.matches("<!-- fastctx:begin -->").count(), 1);
        assert_eq!(agents.matches("<!-- fastctx:end -->").count(), 1);
        assert_eq!(agents.contains("### Shell commands"), fastshell);
        assert!(agents.contains("### Batch replacement"), "{agents}");
        assert!(agents.contains("mcp__fastctx__replace"), "{agents}");
        for removed in ["copy", "cut", "paste", "clips", "drop"] {
            assert!(!agents.contains(&format!("mcp__fastctx__{removed}")));
        }
        let status = isolated_command(temp.path())
            .arg("status")
            .output()
            .unwrap();
        assert_success(&status);
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(status.contains("[PASS] MCP handshake"), "{status}");
        let prefix = if fastshell { "[PASS]" } else { "[INFO]" };
        assert!(status.contains(&format!("{prefix} fastshell")), "{status}");
        assert!(!status.contains("fastshell bash"), "{status}");
        assert!(!status.contains("fastedit"), "{status}");

        let removed = isolated_command(temp.path())
            .args(["unapply", "--yes"])
            .output()
            .unwrap();
        assert_success(&removed);
        assert!(!temp.path().join(".codex").exists());
        assert!(!temp.path().join(".fastctx").exists());
    }
}

#[test]
fn reapply_removes_a_disabled_shell_flag_from_the_single_server() {
    let temp = tempfile::tempdir().unwrap();
    write_shell_settings(temp.path(), true);
    let first = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .output()
        .unwrap();
    assert_success(&first);

    let settings_path = temp.path().join(".fastctx/config.toml");
    let mut settings = fastctx::control::settings::load_from(&settings_path).unwrap();
    settings.fastshell.enabled = false;
    std::fs::write(
        &settings_path,
        fastctx::control::settings::encode(&settings).unwrap(),
    )
    .unwrap();
    let second = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .output()
        .unwrap();
    assert_success(&second);
    let preview = String::from_utf8_lossy(&second.stdout);
    assert!(preview.contains("[mcp_servers.fastctx] args"), "{preview}");
    assert!(!preview.contains("--enable-shell"), "{preview}");
    let config = std::fs::read_to_string(temp.path().join(".codex/config.toml")).unwrap();
    assert!(config.contains("args = [\"serve\"]"), "{config}");
    assert_eq!(config.matches("mcp__fastctx").count(), 1);
    let agents = std::fs::read_to_string(temp.path().join(".codex/AGENTS.md")).unwrap();
    assert!(!agents.contains("### Shell commands"));
    assert!(agents.contains("### Batch replacement"));
    assert!(agents.contains("mcp__fastctx__replace"));
}

#[test]
fn apply_migrates_owned_three_server_config_and_legacy_agents_blocks_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    write_legacy_extension_settings(temp.path(), true, true);

    let installed = temp.path().join(".fastctx/bin").join(if cfg!(windows) {
        "fastctx.exe"
    } else {
        "fastctx"
    });
    let legacy_read = temp.path().join(".fastread/bin").join(if cfg!(windows) {
        "fastread.exe"
    } else {
        "fastread"
    });
    let config = format!(
        concat!(
            "# user prefix\n",
            "[mcp_servers.fastread]\n",
            "command = '{legacy_read}'\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastread.env]\n",
            "FASTREAD_TOKEN_BUDGET = '8500'\n\n",
            "[mcp_servers.fastctx]\n",
            "command = '{installed}'\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastctx.env]\n",
            "FASTCTX_TOKEN_BUDGET = '8500'\n\n",
            "[mcp_servers.fastshell]\n",
            "command = '{installed}'\n",
            "args = ['shell-serve']\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastshell.env]\n",
            "FASTSHELL_TOKEN_BUDGET = '8500'\n\n",
            "[mcp_servers.fastedit]\n",
            "command = '{installed}'\n",
            "args = ['edit-serve']\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastedit.env]\n",
            "FASTEDIT_TOKEN_BUDGET = '8500'\n\n",
            "[mcp_servers.user_owned]\n",
            "command = 'keep-me'\n\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = ['user', 'mcp__fastread', 'mcp__fastctx', 'mcp__fastshell', 'mcp__fastedit']\n"
        ),
        legacy_read = normalized(&legacy_read),
        installed = normalized(&installed),
    );
    std::fs::write(codex.join("config.toml"), config).unwrap();
    let agents = concat!(
        "# user rules\n\n",
        "<!-- fastread:begin -->\n",
        "## Local file inspection\n\n",
        "The fastread MCP tools are the first-class way to read, search, and find\n",
        "local files: `mcp__fastread__read`, `mcp__fastread__grep`,\n",
        "`mcp__fastread__glob` — prefer them over `cat`/`Get-Content`,\n",
        "`rg`/`findstr`/`Select-String`, and `dir`/`ls -R`. Pass absolute paths. The\n",
        "last line of every result says `Complete` or `Partial` — continue only with\n",
        "the exact parameters a `Partial` note provides.\n",
        "<!-- fastread:end -->\n\n",
        "<!-- fastctx:begin -->\n### Bulk edits and moving code\nUse mcp__fastctx__copy then mcp__fastctx__paste.\n<!-- fastctx:end -->\n\n",
        "user suffix\n"
    );
    std::fs::write(codex.join("AGENTS.md"), agents).unwrap();

    let output = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .output()
        .unwrap();
    assert_success(&output);
    let preview = String::from_utf8_lossy(&output.stdout);
    for removed in [
        "- [mcp_servers.fastread]",
        "- [mcp_servers.fastshell]",
        "- [mcp_servers.fastedit]",
        "- direct_only_tool_namespaces -= \"mcp__fastread\"",
        "- direct_only_tool_namespaces -= \"mcp__fastshell\"",
        "- direct_only_tool_namespaces -= \"mcp__fastedit\"",
        "- <!-- fastread:begin --> … <!-- fastread:end -->",
    ] {
        assert!(preview.contains(removed), "missing {removed}:\n{preview}");
    }

    let config = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    assert!(config.contains("# user prefix"), "{config}");
    assert!(config.contains("[mcp_servers.user_owned]"), "{config}");
    assert!(config.contains("command = 'keep-me'"), "{config}");
    assert!(config.contains("[mcp_servers.fastctx]"), "{config}");
    assert!(
        config.contains("args = [\"serve\", \"--enable-shell\"]"),
        "{config}"
    );
    for legacy in [
        "[mcp_servers.fastread]",
        "[mcp_servers.fastshell]",
        "[mcp_servers.fastedit]",
        "mcp__fastread",
        "mcp__fastshell",
        "mcp__fastedit",
    ] {
        assert!(!config.contains(legacy), "{legacy} survived:\n{config}");
    }
    assert_eq!(config.matches("mcp__fastctx").count(), 1, "{config}");

    let agents = std::fs::read_to_string(codex.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# user rules\n\n"), "{agents}");
    assert!(agents.ends_with("\nuser suffix\n"), "{agents}");
    assert!(!agents.contains("<!-- fastread:begin -->"), "{agents}");
    assert_eq!(agents.matches("<!-- fastctx:begin -->").count(), 1);
    assert!(agents.contains("### Shell commands"), "{agents}");
    assert!(agents.contains("### Batch replacement"), "{agents}");
    assert!(agents.contains("mcp__fastctx__replace"), "{agents}");
    assert!(!agents.contains("mcp__fastctx__copy"), "{agents}");
    assert!(!agents.contains("mcp__fastctx__paste"), "{agents}");
}

#[test]
fn fastshell_preflight_failure_leaves_every_target_byte_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    let config = b"# user config\ntool_output_token_limit = 10000\n";
    let agents = b"# user agents\n";
    std::fs::write(codex.join("config.toml"), config).unwrap();
    std::fs::write(codex.join("AGENTS.md"), agents).unwrap();
    write_shell_settings(temp.path(), true);
    let missing = normalized(&temp.path().join("missing-bash"));
    let output = isolated_command(temp.path())
        .args(["apply", "--yes"])
        .env("FASTCTX_BASH", &missing)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("fastshell is enabled"), "{error}");
    assert!(error.contains("Invalid FASTCTX_BASH"), "{error}");
    assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), config);
    assert_eq!(std::fs::read(codex.join("AGENTS.md")).unwrap(), agents);
    assert!(!temp.path().join(".fastctx/bin").exists());
}

#[test]
fn noninteractive_apply_refuses_a_non_directory_codex_profile_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::write(&codex, b"user-owned path").unwrap();

    let output = isolated_command(temp.path())
        .args(["apply", "--tier", "standard", "--yes"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("is not a directory"), "{error}");
    assert_eq!(std::fs::read(&codex).unwrap(), b"user-owned path");
    assert!(!temp.path().join(".fastctx").exists());
}

#[test]
fn a_non_tty_apply_without_yes_refuses_a_shared_limit_conflict_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let codex = temp.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    let config = b"tool_output_token_limit = 7000\n";
    std::fs::write(codex.join("config.toml"), config).unwrap();
    let output = isolated_command(temp.path())
        .args(["apply", "--tier", "extra-high"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Re-run with --yes"));
    assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), config);
    assert!(!temp.path().join(".fastctx").exists());
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fastctx"))
}

fn isolated_command(home: &Path) -> Command {
    let mut command = command();
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("CODEX_HOME")
        .env("TMPDIR", home)
        .env("TMP", home)
        .env("TEMP", home);
    command
}

fn profile_test_home() -> tempfile::TempDir {
    // Canonicalize the base so the temp path's Windows drive-letter case is stable regardless
    // of the shell cwd: the binary echoes back the path it receives while normalized()
    // canonicalizes for comparison, so both sides must agree on drive case (2026-07-22).
    let base = dunce::canonicalize(std::env::current_dir().unwrap()).unwrap();
    tempfile::Builder::new()
        .prefix("fastctx-codex-home-")
        .tempdir_in(base)
        .unwrap()
}

fn start_persistent_job(home: &Path, command: &str) -> String {
    let mut server = isolated_command(home);
    server.args(["serve", "--enable-shell"]);
    let mut session = McpSession::start(server);
    let started = session.call(
        "run_background",
        serde_json::json!({
            "command": command,
            "cwd": normalized(home),
            "login_shell": false
        }),
    );
    let job_id = mcp_text(&started)
        .strip_prefix("(Complete: job ")
        .and_then(|value| value.strip_suffix(" started.)"))
        .expect("run_background must return its stable job id")
        .to_string();
    assert!(session.close().success());
    job_id
}

struct BackgroundJobCleanup {
    home: PathBuf,
    job_id: Option<String>,
}

impl BackgroundJobCleanup {
    fn new(home: &Path, job_id: &str) -> Self {
        Self {
            home: home.to_path_buf(),
            job_id: Some(job_id.to_string()),
        }
    }

    fn disarm(&mut self) {
        self.job_id = None;
    }
}

impl Drop for BackgroundJobCleanup {
    fn drop(&mut self) {
        if let Some(job_id) = self.job_id.take() {
            let _ = isolated_command(&self.home)
                .args(["jobs", "kill", &job_id])
                .output();
        }
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn backup_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".fastctx-backup-")
        })
        .map(|entry| entry.path().strip_prefix(root).unwrap().to_path_buf())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn write_shell_settings(home: &Path, fastshell: bool) {
    let directory = home.join(".fastctx");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        format!("schema_version = 1\n\n[fastshell]\nenabled = {fastshell}\n"),
    )
    .unwrap();
}

fn write_legacy_extension_settings(home: &Path, fastshell: bool, fastedit: bool) {
    let directory = home.join(".fastctx");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        format!(
            "schema_version = 1\n\n[fastshell]\nenabled = {fastshell}\n\n[fastedit]\nenabled = {fastedit}\n"
        ),
    )
    .unwrap();
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
