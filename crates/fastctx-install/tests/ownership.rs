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
    for original in [None, Some(b"".as_slice()), Some(b"# user\r\n".as_slice())] {
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
    }
    let foreign = b"<!-- fastctx:begin -->\nbody\n<!-- fastctx:end -->\n";
    let file = File { path: "CLAUDE.md", bytes: Some(foreign) };
    assert!(guidance::apply(file, "body", None).is_err());
    assert_eq!(guidance::unapply(file, None).unwrap().bytes.as_deref(), Some(foreign.as_slice()));
}

#[test]
fn native_tools_migrate_old_receipts_and_keep_user_changes() {
    let config = File { path: "config.toml", bytes: Some(b"features.view_image = false\n") };
    let agents = File { path: "AGENTS.md", bytes: None };
    let installed = codex::apply(config, agents, &plan(), "body", Ownership::None, Approval::Require).unwrap();
    let mut old = serde_json::to_value(&installed.receipt).unwrap();
    old.as_object_mut().unwrap().remove("tools");
    let old = serde_json::from_value(old).unwrap();
    let old_config = String::from_utf8(installed.config).unwrap().replace("features.shell_tool = false\n", "");
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
    let applied = tools::apply(file, None).unwrap();
    let deleted = tools::apply(File { bytes: None, ..file }, Some(&applied.receipt)).unwrap();
    assert!(deleted.bytes.is_none());
    assert_eq!(deleted.notices.len(), 3);
    let value: serde_json::Value = serde_json::from_slice(applied.bytes.as_ref().unwrap()).unwrap();
    assert_eq!(value["permissions"]["deny"], serde_json::json!(["Bash", "Grep", "Glob"]));
    let restored = tools::unapply(File { bytes: applied.bytes.as_deref(), ..file }, &applied.receipt).unwrap();
    assert_eq!(restored.bytes.as_deref(), Some(original.as_slice()));

    let edited = String::from_utf8(applied.bytes.unwrap()).unwrap().replace("\"Grep\"", "\"Grep(project)\"");
    let repeated = tools::apply(File { bytes: Some(edited.as_bytes()), ..file }, Some(&applied.receipt)).unwrap();
    assert_eq!(repeated.bytes.as_deref(), Some(edited.as_bytes()));
    assert_eq!(repeated.notices.len(), 1);
    let taken = tools::unapply(File { bytes: repeated.bytes.as_deref(), ..file }, &repeated.receipt).unwrap();
    let value: serde_json::Value = serde_json::from_slice(taken.bytes.as_ref().unwrap()).unwrap();
    assert_eq!(value["permissions"]["deny"], serde_json::json!(["Bash", "Grep(project)"]));
    assert_eq!(value["permissions"]["allow"], serde_json::json!(["Edit", "Write"]));
}
