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
        Err(Error::TokenLimitApproval { current: 8192, requested: 125000, .. })
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
        "tool_output_token_limit = 125000",
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
