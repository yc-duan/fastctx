//! The installation lifecycle is a data-preservation contract.

use fastctx_install::codex::{self, Approval, Ownership};
use fastctx_install::{Error, File, guidance, plan::CodexPlan};

fn plan() -> CodexPlan {
    CodexPlan::new(&std::env::current_dir().unwrap().join("fastctx.exe"))
}

#[test]
fn installation_round_trip_and_drift_are_independent() {
    let original = b"\xef\xbb\xbf# user\r\n tool_output_token_limit = +8_192  # shared\r\n[other]\r\nvalue = '''[mcp_servers.fastctx]'''\r\n";
    let instructions = b"\xff user bytes\r\n";
    let config = File { path: "profile/config.toml", bytes: Some(original) };
    let agents = File { path: "profile/AGENTS.md", bytes: Some(instructions) };
    assert!(matches!(
        codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::Require),
        Err(Error::TokenLimitApproval { current: 8192, requested, .. }) if requested == plan().output_token_limit
    ));
    let applied = codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::RaiseOutputLimit).unwrap();
    let receipt = serde_json::from_slice(&serde_json::to_vec(&applied.receipt).unwrap()).unwrap();
    let repeated = codex::apply(
        File { bytes: Some(&applied.config), ..config },
        File { bytes: Some(&applied.agents), ..agents },
        &plan(),
        "body",
        Ownership::Current(&receipt),
        Approval::Require,
    )
    .unwrap();
    assert_eq!(repeated.config, applied.config);
    assert_eq!(repeated.agents, applied.agents);
    assert_eq!(repeated.receipt, applied.receipt);
    let restored = codex::unapply(
        File { bytes: Some(&applied.config), ..config },
        File { bytes: Some(&applied.agents), ..agents },
        Some(&receipt),
    )
    .unwrap();
    assert_eq!(restored.config.as_deref(), Some(original.as_slice()));
    assert_eq!(restored.agents.as_deref(), Some(instructions.as_slice()));
    assert!(restored.retained.is_empty());
    assert!(restored.receipt.is_none());

    let edited = String::from_utf8(applied.config.clone()).unwrap().replacen(
        &format!("tool_output_token_limit = {}", plan().output_token_limit),
        "tool_output_token_limit = 7",
        1,
    );
    let restored = codex::unapply(
        File { bytes: Some(edited.as_bytes()), ..config },
        File { bytes: Some(&applied.agents), ..agents },
        Some(&receipt),
    )
    .unwrap();
    assert_eq!(restored.retained.len(), 1);
    assert_eq!(restored.agents.as_deref(), Some(instructions.as_slice()));
    assert_eq!(
        String::from_utf8(restored.config.unwrap()).unwrap(),
        String::from_utf8(original.to_vec()).unwrap().replace("+8_192", "7")
    );
    assert!(restored.receipt.is_some());
}

#[test]
fn file_existence_and_unowned_bytes_survive() {
    for original in [None, Some(b"".as_slice()), Some(b"# user\r\n".as_slice()), Some(b"# user".as_slice())] {
        let config = File { path: "config.toml", bytes: original };
        let agents = File { path: "CLAUDE.md", bytes: original };
        let installed = codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::Require).unwrap();
        let restored = codex::unapply(
            File { bytes: Some(&installed.config), ..config },
            File { bytes: Some(&installed.agents), ..agents },
            Some(&installed.receipt),
        )
        .unwrap();
        assert_eq!(restored.config.as_deref(), original);
        assert_eq!(restored.agents.as_deref(), original);

        let tail = b"[user_added]\nvalue = 'preserved'\n";
        let extended = [installed.config.as_slice(), tail].concat();
        let restored = codex::unapply(
            File { bytes: Some(&extended), ..config },
            File { bytes: Some(&installed.agents), ..agents },
            Some(&installed.receipt),
        )
        .unwrap();
        assert!(restored.retained.is_empty());
        let bytes = restored.config.unwrap();
        assert!(bytes.ends_with(tail));
        let value: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(value["user_added"]["value"].as_str(), Some("preserved"));
    }
    let foreign = b"<!-- fastctx:begin -->\nbody\n<!-- fastctx:end -->\n";
    let file = File { path: "CLAUDE.md", bytes: Some(foreign) };
    assert!(guidance::apply(file, "body", None).is_err());
    assert_eq!(guidance::unapply(file, None).unwrap().bytes.as_deref(), Some(foreign.as_slice()));
}

#[test]
fn configuration_rewrites_preserve_ownership_and_user_bytes() {
    let original =
        b"model = 'user'\ntool_output_token_limit = +8_192 # original spelling\nfeatures.shell_tool = true\n";
    let config = File { path: "config.toml", bytes: Some(original) };
    let agents = File { path: "AGENTS.md", bytes: None };
    let installed = codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::RaiseOutputLimit).unwrap();
    let receipt = serde_json::from_slice(&serde_json::to_vec(&installed.receipt).unwrap()).unwrap();
    let mut doc = std::str::from_utf8(&installed.config).unwrap().parse::<toml_edit::DocumentMut>().unwrap();
    let server = doc["mcp_servers"]["fastctx"].as_table().unwrap().clone().into_inline_table();
    doc["mcp_servers"]["fastctx"] = toml_edit::value(server);
    let rewritten = doc.to_string().replace('\n', "\r\n").replace(
        &format!("tool_output_token_limit = {}", plan().output_token_limit),
        &format!("'tool_output_token_limit' = +{}", plan().output_token_limit),
    );
    let user = "\r\n[untouched] # retained exactly\r\n  text = '''# user's text'''  # note\r\n";
    let rewritten = format!("{rewritten}{user}");
    let repeated = codex::apply(
        File { bytes: Some(rewritten.as_bytes()), ..config },
        File { bytes: Some(&installed.agents), ..agents },
        &plan(),
        "body",
        Ownership::Current(&receipt),
        Approval::Require,
    )
    .unwrap();
    assert_eq!(repeated.config, rewritten.as_bytes());
    assert!(repeated.notices.is_empty());
    for receipt in [&receipt, &repeated.receipt] {
        let restored = codex::unapply(
            File { bytes: Some(rewritten.as_bytes()), ..config },
            File { bytes: Some(&installed.agents), ..agents },
            Some(receipt),
        )
        .unwrap();
        assert!(restored.retained.is_empty());
        assert!(restored.receipt.is_none());
        let bytes = restored.config.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("model = 'user'\r\n"));
        assert!(text.contains("'tool_output_token_limit' = +8_192 # original spelling\r\n"));
        assert!(text.ends_with(user));
        let parsed: toml::Value = toml::from_str(text).unwrap();
        assert!(parsed["mcp_servers"].get("fastctx").is_none());
        assert_eq!(parsed["features"]["shell_tool"].as_bool(), Some(true));
        assert!(parsed["features"].get("view_image").is_none());
    }
}

#[test]
fn owned_output_limits_follow_the_plan_and_restore_user_values() {
    let current = plan();
    let mut old_plan = current.clone();
    old_plan.output_token_limit += 2;
    for before in [
        None,
        Some(current.output_token_limit - 1),
        Some(current.output_token_limit),
        Some(current.output_token_limit + 1),
    ] {
        let setting = before.map(|value| format!("tool_output_token_limit = +{value} # user\n")).unwrap_or_default();
        let original = format!("model = 'user'\n{setting}");
        let config = File { path: "config.toml", bytes: Some(original.as_bytes()) };
        let agents = File { path: "AGENTS.md", bytes: None };
        let old = codex::apply(config, agents, &old_plan, "body", Ownership::None, Approval::RaiseOutputLimit).unwrap();
        let receipt = serde_json::from_slice(&serde_json::to_vec(&old.receipt).unwrap()).unwrap();
        let next = codex::apply(
            File { bytes: Some(&old.config), ..config },
            File { bytes: Some(&old.agents), ..agents },
            &current,
            "body",
            Ownership::Current(&receipt),
            Approval::Require,
        )
        .unwrap();
        assert!(next.notices.is_empty());
        let value: toml::Value = toml::from_str(std::str::from_utf8(&next.config).unwrap()).unwrap();
        let sufficient = before.filter(|value| *value >= current.output_token_limit);
        assert_eq!(
            value["tool_output_token_limit"].as_integer(),
            Some(sufficient.unwrap_or(current.output_token_limit) as i64)
        );
        for name in ["run", "apply_patch", "ask_colleague", "message_colleague"] {
            assert_eq!(
                value["mcp_servers"]["fastctx"]["tools"][name]["output_token_limit"].as_integer(),
                Some(current.output_token_limit as i64)
            );
        }
        assert_eq!(serde_json::to_value(&next.receipt).unwrap()["limit"].is_null(), sufficient.is_some());
        let repeated = codex::apply(
            File { bytes: Some(&next.config), ..config },
            File { bytes: Some(&next.agents), ..agents },
            &current,
            "body",
            Ownership::Current(&next.receipt),
            Approval::Require,
        )
        .unwrap();
        assert_eq!(repeated.config, next.config);
        assert_eq!(repeated.receipt, next.receipt);
        let restored = codex::unapply(
            File { bytes: Some(&next.config), ..config },
            File { bytes: Some(&next.agents), ..agents },
            Some(&next.receipt),
        )
        .unwrap();
        assert!(restored.retained.is_empty());
        assert!(restored.receipt.is_none());
        assert_eq!(restored.config.as_deref(), Some(original.as_bytes()));
    }
}

#[test]
fn native_tools_migrate_old_receipts_and_keep_user_changes() {
    let config = File { path: "config.toml", bytes: Some(b"features.view_image = false\n") };
    let agents = File { path: "AGENTS.md", bytes: None };
    let installed = codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::Require).unwrap();
    let mut old = serde_json::to_value(&installed.receipt).unwrap();
    old.as_object_mut().unwrap().remove("tools");
    let old = serde_json::from_value(old).unwrap();
    let old_config = String::from_utf8(installed.config)
        .unwrap()
        .replace("features.shell_tool = false\n", "")
        .replace("features.goals = false\n", "")
        .replace("tools.update_plan.enabled = false\n", "");
    let installed = codex::apply(
        File { bytes: Some(old_config.as_bytes()), ..config },
        File { bytes: Some(&installed.agents), ..agents },
        &plan(),
        "body",
        Ownership::Current(&old),
        Approval::Require,
    )
    .unwrap();
    let changed = String::from_utf8(installed.config)
        .unwrap()
        .replace("features.shell_tool = false", "features.shell_tool = true");
    let reapplied = codex::apply(
        File { bytes: Some(changed.as_bytes()), ..config },
        File { bytes: Some(&installed.agents), ..agents },
        &plan(),
        "body",
        Ownership::Current(&installed.receipt),
        Approval::Require,
    )
    .unwrap();
    assert_eq!(reapplied.config, changed.as_bytes());
    assert_eq!(reapplied.notices.len(), 1);
    let taken = codex::unapply(
        File { bytes: Some(&reapplied.config), ..config },
        File { bytes: Some(&reapplied.agents), ..agents },
        Some(&reapplied.receipt),
    )
    .unwrap();
    assert_eq!(taken.config.as_deref(), Some(b"features.shell_tool = true\nfeatures.view_image = false\n".as_slice()));
    assert!(taken.receipt.is_none());
}

#[test]
fn claude_native_deny_ownership_is_per_rule() {
    use fastctx_install::claude::tools;
    let original = br#"{"permissions":{"deny":["Bash"],"allow":["Edit","Write"]},"user":true}"#;
    let file = File { path: "settings.json", bytes: Some(original) };
    let applied = tools::apply(file, None, false).unwrap();
    let deleted = tools::apply(File { bytes: None, ..file }, Some(&applied.receipt), false).unwrap();
    assert!(deleted.bytes.is_none());
    assert_eq!(deleted.notices.len(), 9);
    let value: serde_json::Value = serde_json::from_slice(applied.bytes.as_ref().unwrap()).unwrap();
    assert_eq!(
        value["permissions"]["deny"],
        serde_json::json!([
            "Bash",
            "Grep",
            "Glob",
            "TodoWrite",
            "TaskCreate",
            "TaskUpdate",
            "TaskList",
            "TaskGet",
            "ProposeGoal"
        ])
    );
    let restored = tools::unapply(File { bytes: applied.bytes.as_deref(), ..file }, &applied.receipt).unwrap();
    assert_eq!(restored.bytes.as_deref(), Some(original.as_slice()));

    let edited = String::from_utf8(applied.bytes.unwrap()).unwrap().replace("\"Grep\"", "\"Grep(project)\"");
    let repeated =
        tools::apply(File { bytes: Some(edited.as_bytes()), ..file }, Some(&applied.receipt), false).unwrap();
    assert_eq!(repeated.bytes.as_deref(), Some(edited.as_bytes()));
    assert_eq!(repeated.notices.len(), 1);
    let taken = tools::unapply(File { bytes: repeated.bytes.as_deref(), ..file }, &repeated.receipt).unwrap();
    let value: serde_json::Value = serde_json::from_slice(taken.bytes.as_ref().unwrap()).unwrap();
    assert_eq!(value["permissions"]["deny"], serde_json::json!(["Bash", "Grep(project)"]));
    assert_eq!(value["permissions"]["allow"], serde_json::json!(["Edit", "Write"]));
}

#[test]
fn portable_server_ownership_survives_machine_changes_and_withdrawal() {
    let original = b"# user words\r\nmodel = 'user'\r\n";
    let config = File { path: "config.toml", bytes: Some(original) };
    let agents = File { path: "AGENTS.md", bytes: None };
    let first_plan = plan();
    let second_plan = CodexPlan::new(&std::env::current_dir().unwrap().join("other-machine/fastctx.exe"));
    let installed = codex::apply(config, agents, &first_plan, "body", Ownership::None, Approval::Require).unwrap();
    let mut doc = std::str::from_utf8(&installed.config).unwrap().parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["mcp_servers"]["fastctx"]["command"].as_str(), first_plan.command.to_str());
    doc["mcp_servers"]["fastctx"]["command"] = toml_edit::value("fastctx");
    let command = toml_edit::Value::from(first_plan.command.to_str().unwrap()).to_string();
    let shared = std::str::from_utf8(&installed.config).unwrap().replacen(
        &format!("command = {command}"),
        "command = \"fastctx\"",
        1,
    );
    let first = codex::apply(
        File { bytes: Some(shared.as_bytes()), ..config },
        File { bytes: Some(&installed.agents), ..agents },
        &first_plan,
        "body",
        Ownership::Current(&installed.receipt),
        Approval::Require,
    )
    .unwrap();
    assert_eq!(first.config, shared.as_bytes());
    let second = codex::apply(
        File { bytes: Some(shared.as_bytes()), ..config },
        agents,
        &second_plan,
        "body",
        Ownership::None,
        Approval::Require,
    )
    .unwrap();
    assert_eq!(second.config, shared.as_bytes());
    let recorded = serde_json::to_value(&second.receipt).unwrap();
    let body: Vec<u8> = serde_json::from_value(recorded["server"]["body"].clone()).unwrap();
    let recorded: toml::Value = toml::from_str(std::str::from_utf8(&body).unwrap()).unwrap();
    assert_eq!(recorded["mcp_servers"]["fastctx"]["command"].as_str(), Some("fastctx"));

    // Withdrawal on either machine must accept the portable command even
    // before that machine has updated its original absolute-path receipt.
    for receipt in [&installed.receipt, &first.receipt, &second.receipt] {
        let taken = codex::unapply(File { bytes: Some(shared.as_bytes()), ..config }, agents, Some(receipt)).unwrap();
        let text = std::str::from_utf8(taken.config.as_ref().unwrap()).unwrap();
        assert!(text.contains(std::str::from_utf8(original).unwrap()));
        let parsed: toml::Value = toml::from_str(text).unwrap();
        assert!(parsed.get("mcp_servers").and_then(|servers| servers.get("fastctx")).is_none());
        assert_eq!(taken.messages.len(), 1);
        let again = codex::apply(
            File { bytes: taken.config.as_deref(), ..config },
            agents,
            &second_plan,
            "body",
            Ownership::Current(taken.receipt.as_ref().unwrap()),
            Approval::Require,
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(std::str::from_utf8(&again.config).unwrap()).unwrap();
        assert_eq!(parsed["mcp_servers"]["fastctx"]["command"].as_str(), Some("fastctx"));
        assert_eq!(parsed["features"]["shell_tool"].as_bool(), Some(false));
    }

    for command in ["fastctx.exe", "FASTCTX", "./fastctx", "other-server", second_plan.command.to_str().unwrap()] {
        let mut wrong = doc.clone();
        wrong["mcp_servers"]["fastctx"]["command"] = toml_edit::value(command);
        let wrong = wrong.to_string();
        assert!(
            codex::apply(
                File { bytes: Some(wrong.as_bytes()), ..config },
                agents,
                &second_plan,
                "body",
                Ownership::None,
                Approval::Require
            )
            .unwrap_err()
            .to_string()
            .contains("command = \"fastctx\"")
        );
    }
    for ownership in [Ownership::None, Ownership::Current(&first.receipt)] {
        let mut wrong = doc.clone();
        wrong["mcp_servers"]["fastctx"]["args"] = toml_edit::value(toml_edit::Array::from_iter(["serve", "user"]));
        let wrong = wrong.to_string();
        assert!(
            codex::apply(
                File { bytes: Some(wrong.as_bytes()), ..config },
                agents,
                &second_plan,
                "body",
                ownership,
                Approval::Require
            )
            .is_err()
        );
    }
}
