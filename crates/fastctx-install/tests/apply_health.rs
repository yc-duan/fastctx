//! Exercise disk orchestration as well as the pure ownership planners.

use std::fs;

use fastctx_install::apply::{self, Hosts, Places};
use fastctx_install::codex::Approval;
use sha2::{Digest, Sha256};

#[test]
fn released_ownership_is_bound_and_retired_with_the_host_files() {
    // Apply ends by asking the machine's Codex about its tools, and a Codex
    // that starts writes into its home; this test's homes must stay its own.
    // SAFETY: the only test in this binary, before it starts any thread.
    unsafe { std::env::set_var("PATH", "") };
    let temp = tempfile::tempdir().unwrap();
    let mut places = Places {
        root: temp.path().join("fastctx"),
        world: temp.path().join("fastctx/world"),
        codex_home: temp.path().join("codex"),
        claude_home: temp.path().join("claude"),
        claude: None,
        fastctx: std::env::current_exe().unwrap(),
    };
    fs::create_dir(&places.root).unwrap();
    fs::create_dir(&places.codex_home).unwrap();
    let config = places.codex_home.join("config.toml");
    let agents = places.codex_home.join("AGENTS.md");
    let quote = |path: &std::path::Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let old_config = format!(
        "# user's model\nmodel = \"user\"\ntool_output_token_limit = 10000\n\n\
         [mcp_servers.fastctx]\ncommand = {}\nargs = [\"serve\"]\nstartup_timeout_sec = 120\n\n\
         [mcp_servers.fastctx.env]\nFASTCTX_TOKEN_BUDGET = \"8500\"\n",
        quote(&places.installed())
    );
    let mut old_agents = include_bytes!("../src/legacy_guidance/v010.txt").to_vec();
    old_agents.push(b'\n');
    let receipt = format!(
        "schema_version = 1\n# user's settings\nshow_hints = true\n\
         [applied]\nversion = \"0.1.0\"\ncommand = {}\n\
         tool_output_token_limit = 10000\nprevious_token_limit_present = false\nfastctx_token_budget = 8500\n\
         [applied.codex_config]\npath = {}\noriginal_existed = true\napplied_sha256 = \"{:x}\"\n\
         [applied.codex_agents]\npath = {}\noriginal_existed = false\napplied_sha256 = \"{:x}\"\n",
        quote(&places.installed()),
        quote(&config),
        Sha256::digest(old_config.as_bytes()),
        quote(&agents),
        Sha256::digest(&old_agents),
    );
    let hosts = Hosts { codex: true, claude: false };
    for migrate in [false, true] {
        fs::write(&config, &old_config).unwrap();
        fs::write(&agents, &old_agents).unwrap();
        fs::write(places.root.join("config.toml"), &receipt).unwrap();
        if migrate {
            apply::apply(&places, hosts, Approval::RaiseOutputLimit).unwrap();
            let retired = fs::read(places.root.join("config.toml")).unwrap();
            fs::remove_file(places.root.join("config.toml")).unwrap();
            fs::create_dir(places.root.join("config.toml")).unwrap();
            apply::apply(&places, hosts, Approval::Require).unwrap();
            apply::unapply(&places, hosts).unwrap();
            fs::remove_dir(places.root.join("config.toml")).unwrap();
            fs::write(places.root.join("config.toml"), retired).unwrap();
        } else {
            apply::unapply(&places, hosts).unwrap();
        }
        assert!(!agents.exists());
        let remaining = fs::read_to_string(&config).unwrap();
        assert!(remaining.starts_with("# user's model\nmodel = \"user\"\n"));
        assert!(!remaining.contains("fastctx"));
        assert!(!remaining.contains("tool_output_token_limit"));
        let remaining = fs::read_to_string(places.root.join("config.toml")).unwrap();
        assert!(remaining.contains("# user's settings\nshow_hints = true"));
        assert!(!remaining.contains("[applied"));
        assert!(!places.root.join("applied.json").exists());
    }
    places.codex_home = temp.path().join("another-profile");
    apply::apply(&places, hosts, Approval::Require).unwrap();
    apply::unapply(&places, hosts).unwrap();
    assert!(!places.codex_home.exists());
}
