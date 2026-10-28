//! Frozen public shapes and integer parsing from Claude Code 2.1.287.
//! All paths and installations in these fixtures are synthetic.

use std::path::Path;

use fastctx_install::claude::{
    plugins::{PluginApply, PluginState},
    settings::{self, SettingsOutcome},
};
use fastctx_install::plan::ClaudePlan;
use serde_json::json;

fn plan() -> ClaudePlan {
    ClaudePlan::new(if cfg!(windows) { Path::new("C:/fixture/world") } else { Path::new("/fixture/world") })
}

#[test]
fn claude_2_1_287_public_lists() {
    let plan = plan();
    let plugins = json!([{
        "id": "fastctx-world@fastctx", "version": plan.plugin_version, "scope": "user", "enabled": true,
        "installPath": plan.marketplace.parent().unwrap().join("unrelated-cache-path"),
        "installedAt": "2026-10-03T00:00:00.000Z", "lastUpdated": "2026-10-03T00:00:00.000Z",
        "projectEnabled": false, "mcpServers": {}, "hasUserConfig": false
    }]);
    let markets =
        json!([{"name":"fastctx", "source":"directory", "path":plan.marketplace, "installLocation":plan.marketplace}]);
    let state =
        PluginState::parse(&serde_json::to_vec(&plugins).unwrap(), &serde_json::to_vec(&markets).unwrap()).unwrap();
    assert!(PluginApply::prepare(&plan, state.clone(), None).unwrap().commands().is_empty());
    let mut disabled = state;
    disabled.installations[0].enabled = false;
    let enable = PluginApply::prepare(&plan, disabled, None).unwrap().commands();
    assert_eq!(enable.len(), 1);
    assert_eq!(enable[0].args, ["plugin", "enable", "fastctx-world@fastctx", "--scope", "user"]);
    for wrong in [b"{}".as_slice(), b"null", b"[{}]", b"[{\"id\":\"fastctx-world@fastctx\",\"scope\":\"user\"}]"] {
        assert!(PluginState::parse(wrong, b"[]").is_err());
    }
    assert!(PluginState::parse(b"[]", b"{}").is_err());
}

#[test]
fn claude_2_1_287_effective_output_limit() {
    // The fixtures pin Claude's parsing, not the World's own limit, so the
    // limit they are judged against is fixed here.
    let plan = ClaudePlan { max_mcp_output_tokens: 200_000, ..plan() };
    // Values are accepted/coerced by Claude's settings schema, then its
    // integer parser. The booleans here say whether 200,000 is already met.
    for (raw, sufficient) in [
        ("200000", true),
        ("200000.5", true),
        ("2e5", true),
        ("1e100", true),
        (r#""2e5""#, true),
        (r#"".2e6""#, true),
        (r#""200,000""#, true),
        (r#""200_000""#, true),
        ("\"200\u{a0}000\"", true),
        ("\"200\u{202f}000\"", true),
        (r#""200000junk""#, true),
        (r#""200000.9""#, true),
        (r#""199999.9""#, false),
        (r#""200,00""#, false),
        (r#""20_000,000""#, false),
        (r#""2.000001e5""#, false),
        (r#""0x40000""#, false),
        (r#""1e999""#, false),
        (r#""-300000""#, false),
        ("[200000]", true),
        ("[200,0]", false),
        ("[200,100]", true),
        ("null", false),
        ("false", false),
        ("{}", false),
    ] {
        let original = format!("{{\"env\":{{\"MAX_MCP_OUTPUT_TOKENS\":{raw}}}}}");
        let edit = settings::apply(Some(original.as_bytes()), &plan, None, false).unwrap();
        assert_eq!(edit.outcome == SettingsOutcome::Sufficient, sufficient, "{raw}");
        let restored = settings::unapply(edit.bytes.as_deref(), edit.receipt.as_ref()).unwrap();
        assert_eq!(restored.bytes.as_deref(), Some(original.as_bytes()), "{raw}");
    }
}
