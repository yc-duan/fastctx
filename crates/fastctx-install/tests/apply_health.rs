//! Exercise disk orchestration as well as the pure ownership planners.

use std::fs;

use fastctx_install::apply::{self, Hosts, Places};
use fastctx_install::codex::Approval;
use sha2::{Digest, Sha256};

fn main() {
    if std::env::args().skip(1).eq(["renew", "identify"]) {
        println!("{}", serde_json::to_string(&fastctx_install::renewal::build().unwrap()).unwrap());
        return;
    }
    released_ownership_is_bound_and_retired_with_the_host_files();
    println!("apply ownership, shared configuration and home lifecycle passed");
}

fn released_ownership_is_bound_and_retired_with_the_host_files() {
    // Apply ends by asking the machine's Codex about its tools, and a Codex
    // that starts writes into its home; this test's homes must stay its own.
    // The program is found on PATH, or as the default Codex home records the
    // desktop app's; both point into this test's own empty places.
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("default-codex")).unwrap();
    // SAFETY: the only test in this binary, before it starts any thread.
    unsafe {
        std::env::set_var("PATH", "");
        std::env::set_var("CODEX_HOME", temp.path().join("default-codex"));
    }
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
    home_lifecycle(&places);
    shared_lifecycle(&places);
}

fn shared_lifecycle(base: &Places) {
    use fastctx_install::apply::{Host, Standing};
    let hosts = Host::Codex.only();
    let first = base.at_home(Host::Codex, &base.root.parent().unwrap().join("shared-home")).unwrap();
    let mut second = first.clone();
    second.root = base.root.parent().unwrap().join("second-machine");
    second.world = second.root.join("world");
    fs::create_dir(&first.codex_home).unwrap();
    let config = first.codex_home.join("config.toml");
    let words = b"# user\r\nmodel = 'user'\r\n";
    fs::write(&config, words).unwrap();
    apply::apply(&first, hosts, Approval::Require).unwrap();
    let text = fs::read_to_string(&config).unwrap();
    let command = toml_edit::Value::from(first.installed().to_str().unwrap()).to_string();
    fs::write(&config, text.replacen(&format!("command = {command}"), "command = \"fastctx\"", 1)).unwrap();
    let shared = fs::read(&config).unwrap();
    let report = apply::apply(&first, hosts, Approval::Require).unwrap();
    assert!(report.messages.iter().any(|message| message.contains("not found")));
    assert_eq!(fs::read(&config).unwrap(), shared);

    // Only config.toml is shared; each machine keeps its own instructions.
    second.codex_home = first.codex_home.with_file_name("second-home");
    fs::create_dir(&second.codex_home).unwrap();
    fs::write(second.codex_home.join("config.toml"), &shared).unwrap();
    apply::apply(&second, hosts, Approval::Require).unwrap();
    let report = apply::unapply(&first, hosts).unwrap();
    assert_eq!(report.messages.len(), 1);
    assert_eq!(apply::status_at(&first, Host::Codex, &first.codex_home).unwrap(), Standing::Out);
    assert!(fs::read(&config).unwrap().starts_with(words));
    fs::copy(&config, second.codex_home.join("config.toml")).unwrap();
    apply::apply(&second, hosts, Approval::Require).unwrap();
    let parsed: toml::Value =
        toml::from_str(&fs::read_to_string(second.codex_home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(parsed["mcp_servers"]["fastctx"]["command"].as_str(), Some("fastctx"));
    assert_eq!(parsed["features"]["shell_tool"].as_bool(), Some(false));
    apply::unapply(&second, hosts).unwrap();
    apply::apply(&first, hosts, Approval::Require).unwrap();
    let parsed: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(parsed["mcp_servers"]["fastctx"]["command"].as_str(), Some("fastctx"));
    apply::unapply(&first, hosts).unwrap();
}

fn home_lifecycle(base: &Places) {
    use fastctx_install::apply::{Host, Standing};

    let first = base.at_home(Host::Codex, &base.root.parent().unwrap().join("first-home")).unwrap();
    let second = base.at_home(Host::Codex, &base.root.parent().unwrap().join("new-parent/second-home")).unwrap();
    let third = base.at_home(Host::Codex, &second.codex_home.parent().unwrap().join("third-home")).unwrap();
    fs::create_dir(&first.codex_home).unwrap();
    let original =
        b"# user\r\nmodel = 'first'\r\ntool_output_token_limit = 10000\r\n[features]\r\nshell_tool = true\r\n";
    fs::write(first.codex_home.join("config.toml"), original).unwrap();
    fs::write(first.codex_home.join("AGENTS.md"), b"first user's words\r\n").unwrap();
    let hosts = Host::Codex.only();
    apply::apply(&first, hosts, Approval::RaiseOutputLimit).unwrap();

    let record_path = first.root.join("applied.json");
    let registry: serde_json::Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    let mut old = registry["homes"][0]["receipt"].clone();
    old["created"] = registry["created"].clone();
    fs::write(&record_path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
    assert_eq!(apply::status_at(base, Host::Codex, &first.codex_home).unwrap(), Standing::In);
    assert_eq!(apply::status_at(base, Host::Codex, &second.codex_home).unwrap(), Standing::Absent);
    let first_config = fs::read(first.codex_home.join("config.toml")).unwrap();
    apply::apply(&second, hosts, Approval::Require).unwrap();
    apply::apply(&third, hosts, Approval::Require).unwrap();
    assert_eq!(fs::read(first.codex_home.join("config.toml")).unwrap(), first_config);
    let second_config = fs::read(second.codex_home.join("config.toml")).unwrap();
    let second_agents = fs::read(second.codex_home.join("AGENTS.md")).unwrap();

    let rewritten: toml::Value = toml::from_str(std::str::from_utf8(&first_config).unwrap()).unwrap();
    fs::write(first.codex_home.join("config.toml"), toml::to_string_pretty(&rewritten).unwrap()).unwrap();
    apply::apply(&first, hosts, Approval::Require).unwrap();
    apply::unapply(&first, hosts).unwrap();
    let restored: toml::Value =
        toml::from_str(&fs::read_to_string(first.codex_home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(restored["model"].as_str(), Some("first"));
    assert_eq!(restored["tool_output_token_limit"].as_integer(), Some(10000));
    assert_eq!(restored["features"]["shell_tool"].as_bool(), Some(true));
    assert!(restored.get("mcp_servers").and_then(|servers| servers.get("fastctx")).is_none());
    assert_eq!(fs::read(first.codex_home.join("AGENTS.md")).unwrap(), b"first user's words\r\n");
    assert_eq!(fs::read(second.codex_home.join("config.toml")).unwrap(), second_config);
    assert_eq!(fs::read(second.codex_home.join("AGENTS.md")).unwrap(), second_agents);
    assert_eq!(apply::status_at(base, Host::Codex, &first.codex_home).unwrap(), Standing::Out);
    assert_eq!(apply::status_at(base, Host::Codex, &second.codex_home).unwrap(), Standing::In);
    apply::unapply(&second, hosts).unwrap();
    assert!(!second.codex_home.exists());
    assert!(second.codex_home.parent().unwrap().exists());
    assert_eq!(apply::status_at(base, Host::Codex, &third.codex_home).unwrap(), Standing::In);
    apply::unapply(&third, hosts).unwrap();
    assert!(!second.codex_home.parent().unwrap().exists());
    assert!(!record_path.exists());
}
