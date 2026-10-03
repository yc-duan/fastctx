//! Frozen host configuration and public plugin CLI contracts without either real host.

use fastctx::control::{
    paths::ControlPaths, settings, target_apply, target_status, targets::AgentTarget,
};
use fastctx::server_manifest::EnabledTools;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn options(home: &Path, target: AgentTarget) -> target_apply::TargetApplyOptions {
    target_apply::TargetApplyOptions {
        target,
        enabled_tools: EnabledTools::files(),
        tier: settings::Tier::Standard,
        tool_budgets: settings::ToolBudgetPreferences::default(),
        output_guard_enabled: true,
        current_executable: home.join("source"),
    }
}

#[test]
fn codex_world_budget_contract_and_enrollment_invalidate_preview() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let paths = ControlPaths::for_home(home);
    fs::create_dir_all(&paths.fastctx_dir).unwrap();
    fs::write(home.join("source"), b"fixture binary").unwrap();
    let standalone =
        target_apply::plan_target_apply(&paths, options(home, AgentTarget::Codex)).unwrap();
    fs::write(paths.fastctx_dir.join("world.toml"), b"enrolled").unwrap();
    assert!(
        target_apply::commit_target_apply(standalone, true)
            .unwrap_err()
            .contains("enrollment changed")
    );
    assert!(!paths.codex_config.exists());
    let plan = target_apply::plan_target_apply(&paths, options(home, AgentTarget::Codex)).unwrap();
    target_apply::commit_target_apply(plan, true).unwrap();
    let source = fs::read_to_string(&paths.codex_config).unwrap();
    let config: toml_edit::DocumentMut = source.parse().unwrap();
    assert_eq!(
        config["tool_output_token_limit"].as_integer(),
        Some(125_000)
    );
    assert_eq!(
        config["mcp_servers"]["fastctx"]["tools"]["run"]["output_token_limit"].as_integer(),
        Some(125_000)
    );
    assert_eq!(
        config["mcp_servers"]["fastctx"]["tools"]
            .as_table()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        config["mcp_servers"]["fastctx"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|arg| arg.as_str())
            .collect::<Vec<_>>(),
        [
            "serve",
            "--tools",
            "inspect_local_file,grep,glob,replace",
            "--writer",
            "apply_patch"
        ]
    );
    let plan = target_apply::plan_target_disconnect(&paths, AgentTarget::Codex).unwrap();
    target_apply::commit_target_disconnect(plan).unwrap();
    assert!(!paths.codex_config.exists());
}

fn write_json(path: impl AsRef<Path>, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn claude_public_cli_ownership_and_transaction_contract() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    let source = temp.path().join("fake.rs");
    fs::write(&source, r#"
use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let home = PathBuf::from(env::var_os("HOME").unwrap());
    let config = PathBuf::from(env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    assert_eq!(config, home.join(".claude"));
    let list = match words.as_slice() {
        ["plugin", "list", "--json"] => Some("plugins.json"),
        ["plugin", "marketplace", "list", "--json"] => Some("marketplaces.json"),
        _ => None,
    };
    if let Some(file) = list {
        println!("{}", fs::read_to_string(home.join(file)).unwrap());
        return;
    }
    let stage = match words.as_slice() {
        ["plugin", "marketplace", "add", path, "--scope", "user"] => {
            assert_eq!(PathBuf::from(path), home.join(".fastctx/world/claude-plugins"));
            "add"
        },
        ["plugin", "install", "fastctx-world@fastctx", "--scope", "user"] => "install",
        ["plugin", "uninstall", "fastctx-world@fastctx", "--scope", "user"] => "uninstall",
        ["plugin", "marketplace", "remove", "fastctx", "--scope", "user"] => "remove",
        _ => panic!("unexpected arguments: {args:?}"),
    };
    writeln!(fs::OpenOptions::new().append(true).create(true).open(home.join("calls")).unwrap(), "{stage}").unwrap();
    let world_limit = fs::read_to_string(config.join("settings.json")).unwrap().contains("\"200000\"");
    for file in ["plugins.json", "marketplaces.json", "settings.json"] {
        let mut bytes = fs::read_to_string(home.join("stages").join(stage).join(file)).unwrap();
        let dest = if file == "settings.json" {
            if world_limit { bytes = bytes.replace("\"9000\"", "\"200000\""); }
            config.join(file)
        } else { home.join(file) };
        fs::write(dest, bytes).unwrap();
    }
    if fs::read_to_string(home.join("fail-command")).is_ok_and(|value| value == stage) { std::process::exit(37); }
}
"#).unwrap();
    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    for scenario in [
        "owned",
        "market",
        "both",
        "install_failure",
        "remove_failure",
        "uninstall_failure",
        "missing",
    ] {
        let home = temp.path().join(scenario);
        fs::create_dir_all(home.join(".claude")).unwrap();
        let market = home.join(".fastctx/world/claude-plugins");
        for manifest in [
            market.join(".claude-plugin/marketplace.json"),
            market.join("fastctx-world/.claude-plugin/plugin.json"),
        ] {
            fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            fs::write(manifest, b"{}").unwrap();
        }
        fs::write(home.join(".fastctx/world.toml"), b"enrolled").unwrap();
        fs::write(home.join("source"), b"fixture binary").unwrap();
        // An opaque non-directory makes any attempt to access private plugin files fail.
        fs::write(home.join(".claude/plugins"), b"opaque host state").unwrap();
        for stage in ["add", "install", "uninstall", "remove"] {
            let dir = home.join("stages").join(stage);
            fs::create_dir_all(&dir).unwrap();
            let mut marketplaces =
                json!([null, {"future": [1, 2]}, {"name":"elsewhere","source":"future"}]);
            if stage != "remove" {
                marketplaces.as_array_mut().unwrap().push(json!({"name":"fastctx", "source":"directory", "path":market, "future":{"arbitrary":true}}));
            }
            let mut plugins = json!([42, null, {"future": "shape"}, {"id":"other@elsewhere","scope":"user","enabled":true,"installPath":"opaque"}]);
            if stage == "install" {
                plugins.as_array_mut().unwrap().push(json!({"id":"fastctx-world@fastctx","scope":"user","enabled":true,"installPath":market.join("fastctx-world"),"version":{"unknown":"shape"},"extra":[1,2,3]}));
            }
            write_json(dir.join("plugins.json"), &plugins);
            write_json(dir.join("marketplaces.json"), &marketplaces);
            write_json(
                dir.join("settings.json"),
                &json!({"env":{"MAX_MCP_OUTPUT_TOKENS":"9000"},"userSetting":"kept","cliSetting":stage}),
            );
        }
        let initial = match scenario {
            "market" | "uninstall_failure" => "add",
            "both" => "install",
            _ => "remove",
        };
        for file in ["plugins.json", "marketplaces.json"] {
            fs::copy(
                home.join("stages").join(initial).join(file),
                home.join(file),
            )
            .unwrap();
        }
        fs::write(home.join(".claude/settings.json"), b"{\r\n  \"env\": {\"MAX_MCP_OUTPUT_TOKENS\": \"9000\"}, \"userSetting\": \"kept\"\r\n}\r\n").unwrap();
        let path = if scenario == "missing" {
            home.clone()
        } else {
            temp.path().to_owned()
        };
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "claude_contract_child",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(&home)
            .env("PATH", path)
            .env("CONTRACT_HOME", &home)
            .env("SCENARIO", scenario)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{scenario}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn host_settings(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join(".claude/settings.json")).unwrap()).unwrap()
}

fn calls(home: &Path) -> String {
    fs::read_to_string(home.join("calls")).unwrap_or_default()
}

#[test]
#[ignore = "runs inside the isolated plugin contract process"]
fn claude_contract_child() {
    let home = std::path::PathBuf::from(std::env::var_os("CONTRACT_HOME").unwrap());
    let scenario = std::env::var("SCENARIO").unwrap();
    let paths = ControlPaths::for_home(&home);
    let before_plugins = fs::read(home.join("plugins.json")).unwrap();
    let before_marketplaces = fs::read(home.join("marketplaces.json")).unwrap();
    let plan =
        target_apply::plan_target_apply(&paths, options(&home, AgentTarget::ClaudeCode)).unwrap();
    assert!(calls(&home).is_empty());
    let preview = plan
        .preview()
        .iter()
        .flat_map(|item| &item.details)
        .map(|detail| detail.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if scenario == "missing" {
        assert!(preview.contains("Claude Code was not found"));
    }
    if scenario == "owned" {
        assert!(preview.contains("marketplace add"));
        assert!(preview.contains("--scope user"));
    }
    if scenario == "install_failure" {
        fs::write(home.join("fail-command"), "install").unwrap();
    }
    let result = target_apply::commit_target_apply(plan, true);
    if scenario == "install_failure" {
        assert!(result.unwrap_err().contains("rolled back"));
        assert_eq!(host_settings(&home)["env"]["MAX_MCP_OUTPUT_TOKENS"], "9000");
        assert!(!paths.fastctx_config.exists());
        assert_eq!(fs::read(home.join("plugins.json")).unwrap(), before_plugins);
        assert_eq!(
            fs::read(home.join("marketplaces.json")).unwrap(),
            before_marketplaces
        );
        assert_eq!(calls(&home), "add\ninstall\nremove\n");
        return;
    }
    let result = result.unwrap();
    if scenario == "missing" {
        assert!(
            result
                .notes
                .iter()
                .any(|note| note.contains("Claude Code was not found"))
        );
    }
    assert_eq!(
        host_settings(&home)["env"]["MAX_MCP_OUTPUT_TOKENS"],
        "200000"
    );
    assert_eq!(host_settings(&home)["userSetting"], "kept");
    let settings = settings::load(&paths).unwrap();
    let status = target_status::inspect_target(&paths, &settings, AgentTarget::ClaudeCode);
    assert_eq!(
        status.state,
        target_status::TargetConnectionState::Connected,
        "{:?}",
        status.facts
    );
    if scenario == "missing" {
        assert!(
            status
                .facts
                .iter()
                .any(|fact| fact.contains("Claude Code was not found"))
        );
    }
    if scenario == "owned" {
        let mut legacy = settings.clone();
        legacy
            .applied_targets
            .get_mut(AgentTarget::ClaudeCode.id())
            .unwrap()
            .created_directories
            .push(home.join(".claude/plugins").to_string_lossy().into_owned());
        fs::write(&paths.fastctx_config, settings::encode(&legacy).unwrap()).unwrap();
    }
    let receipt = fs::read(&paths.fastctx_config).unwrap();
    let plan = target_apply::plan_target_disconnect(&paths, AgentTarget::ClaudeCode).unwrap();
    let failure = match scenario.as_str() {
        "remove_failure" => Some("remove"),
        "uninstall_failure" => Some("uninstall"),
        _ => None,
    };
    if let Some(failure) = failure {
        fs::write(home.join("fail-command"), failure).unwrap();
    }
    let result = target_apply::commit_target_disconnect(plan);
    if failure.is_some() {
        assert!(result.unwrap_err().contains("rolled back"));
        assert_eq!(
            host_settings(&home)["env"]["MAX_MCP_OUTPUT_TOKENS"],
            "200000"
        );
        assert_eq!(fs::read(&paths.fastctx_config).unwrap(), receipt);
        let status = target_status::inspect_target(&paths, &settings, AgentTarget::ClaudeCode);
        assert_eq!(
            status.state,
            target_status::TargetConnectionState::Connected,
            "{:?}",
            status.facts
        );
    } else {
        let result = result.unwrap();
        assert!(
            result
                .notes
                .iter()
                .all(|note| !note.contains("Could not remove")),
            "{:?}",
            result.notes
        );
        assert_eq!(host_settings(&home)["env"]["MAX_MCP_OUTPUT_TOKENS"], "9000");
        assert_eq!(host_settings(&home)["userSetting"], "kept");
        assert_eq!(fs::read(home.join("plugins.json")).unwrap(), before_plugins);
        assert_eq!(
            fs::read(home.join("marketplaces.json")).unwrap(),
            before_marketplaces
        );
        assert_eq!(
            calls(&home),
            match scenario.as_str() {
                "owned" => "add\ninstall\nremove\n",
                "market" => "install\nuninstall\n",
                "both" | "missing" => "",
                _ => unreachable!(),
            }
        );
    }
}
