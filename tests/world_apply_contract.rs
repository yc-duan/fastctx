//! Frozen host configuration and plugin CLI contracts, exercised without either real host.

use fastctx::control::{paths::ControlPaths, settings, target_apply, targets::AgentTarget};
use fastctx::server_manifest::EnabledTools;
use serde_json::json;
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

#[test]
fn claude_user_plugin_commands_are_transactional() {
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
    let stage = match words.as_slice() {
        ["plugin", "marketplace", "add", _] => "add",
        ["plugin", "install", "fastctx-world@fastctx", "--scope", "user"] => "install",
        ["plugin", "uninstall", "fastctx-world@fastctx", "--scope", "user"] => "uninstall",
        ["plugin", "marketplace", "remove", "fastctx", "--scope", "user"] => "remove",
        _ => panic!("unexpected arguments: {args:?}"),
    };
    let home = PathBuf::from(env::var_os("HOME").unwrap());
    let config = PathBuf::from(env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    assert_eq!(config, home.join(".claude"));
    let world_limit = fs::read_to_string(config.join("settings.json")).unwrap().contains("\"200000\"");
    writeln!(fs::OpenOptions::new().append(true).create(true).open(home.join("calls")).unwrap(), "{stage}").unwrap();
    for name in ["settings.json", "plugins/known_marketplaces.json", "plugins/installed_plugins.json"] {
        let dest = config.join(name);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let mut bytes = fs::read_to_string(home.join("stages").join(stage).join(name)).unwrap();
        if name == "settings.json" && world_limit { bytes = bytes.replace("\"9000\"", "\"200000\""); }
        fs::write(dest, bytes).unwrap();
    }
    if env::var("FAIL_COMMAND").as_deref() == Ok(stage) { std::process::exit(37); }
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
    for failure in ["none", "install", "uninstall"] {
        let home = temp.path().join(failure);
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".fastctx/world/claude-plugins/fastctx-world/.claude-plugin"))
            .unwrap();
        fs::create_dir_all(home.join(".fastctx/world/claude-plugins/.claude-plugin")).unwrap();
        fs::write(home.join(".fastctx/world.toml"), b"enrolled").unwrap();
        fs::write(home.join("source"), b"fixture binary").unwrap();
        let market = home.join(".fastctx/world/claude-plugins");
        for manifest in [
            market.join(".claude-plugin/marketplace.json"),
            market.join("fastctx-world/.claude-plugin/plugin.json"),
        ] {
            fs::write(manifest, b"{}").unwrap();
        }
        fs::write(
            home.join(".claude/settings.json"),
            b"{\r\n  \"env\": {\"MAX_MCP_OUTPUT_TOKENS\": \"9000\"}\r\n}\r\n",
        )
        .unwrap();
        for stage in ["add", "install", "uninstall", "remove"] {
            let declaration = json!({"source": {"source":"directory", "path": market}});
            let mut settings = json!({"env":{"MAX_MCP_OUTPUT_TOKENS":"9000"}});
            let mut known = json!({});
            let mut installed = json!({"version":2,"plugins":{}});
            if stage != "remove" {
                settings["extraKnownMarketplaces"] = json!({"fastctx":declaration});
                known["fastctx"] = json!({"source":declaration["source"],"installLocation":market,"lastUpdated":"2026-10-02"});
            }
            if stage == "install" {
                settings["enabledPlugins"] = json!({"fastctx-world@fastctx":true});
                installed["plugins"]["fastctx-world@fastctx"] = json!([{"scope":"user","installPath":market.join("fastctx-world"),"version":"1.0.0"}]);
            }
            for (name, value) in [
                ("settings.json", settings),
                ("plugins/known_marketplaces.json", known),
                ("plugins/installed_plugins.json", installed),
            ] {
                let file = home.join("stages").join(stage).join(name);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
            }
        }
        let path = std::env::join_paths(
            std::iter::once(temp.path().to_owned())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "claude_contract_child",
                "--ignored",
                "--nocapture",
            ])
            .env("PATH", path)
            .env("CONTRACT_HOME", &home)
            .env("FAIL_COMMAND", failure)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{failure}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
#[ignore = "runs inside the isolated plugin contract process"]
fn claude_contract_child() {
    let home = std::path::PathBuf::from(std::env::var_os("CONTRACT_HOME").unwrap());
    let paths = ControlPaths::for_home(&home);
    let original = fs::read(home.join(".claude/settings.json")).unwrap();
    let plan =
        target_apply::plan_target_apply(&paths, options(&home, AgentTarget::ClaudeCode)).unwrap();
    assert!(!home.join("calls").exists());
    let result = target_apply::commit_target_apply(plan, true);
    if std::env::var("FAIL_COMMAND").unwrap() == "install" {
        assert!(result.unwrap_err().contains("rolled back"));
        assert_eq!(
            fs::read(home.join(".claude/settings.json")).unwrap(),
            original
        );
        assert!(!paths.fastctx_config.exists());
        assert_eq!(
            fs::read_to_string(home.join("calls")).unwrap(),
            "add\ninstall\nuninstall\nremove\n"
        );
        return;
    }
    result.unwrap();
    let applied = fs::read(home.join(".claude/settings.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&applied).unwrap();
    assert_eq!(value["env"]["MAX_MCP_OUTPUT_TOKENS"], "200000");
    let receipt = fs::read(&paths.fastctx_config).unwrap();
    let plan = target_apply::plan_target_disconnect(&paths, AgentTarget::ClaudeCode).unwrap();
    let result = target_apply::commit_target_disconnect(plan);
    if std::env::var("FAIL_COMMAND").unwrap() == "uninstall" {
        assert!(result.unwrap_err().contains("rolled back"));
        assert_eq!(
            fs::read(home.join(".claude/settings.json")).unwrap(),
            applied
        );
        assert_eq!(fs::read(&paths.fastctx_config).unwrap(), receipt);
    } else {
        result.unwrap();
        assert_eq!(
            fs::read(home.join(".claude/settings.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::read_to_string(home.join("calls")).unwrap(),
            "add\ninstall\nuninstall\nremove\n"
        );
    }
}
