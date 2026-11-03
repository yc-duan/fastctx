//! Every agent connected through its configuration alone, against a home
//! its own program wrote (`tests/fixtures/agents/<name>/`): connecting writes
//! files its format still reads, with Fastctx's server where the agent
//! looks for it; connecting again changes nothing; taking it out gives back
//! every file byte for byte; and its files name who is logged in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fastctx_install::agents::{self, Agent, Files, Format, Server, Slot};
use fastctx_types::logins::Identity;

fn fixture(agent: &Agent) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents").join(agent.name)
}

fn files_of(agent: &Agent, home: &Path) -> Files {
    agent
        .files()
        .into_iter()
        .map(|name| {
            let path = agents::located(home, name);
            (name, (path.display().to_string(), std::fs::read(&path).ok()))
        })
        .collect()
}

fn with(files: &Files, after: &BTreeMap<&'static str, Option<Vec<u8>>>) -> Files {
    files.iter().map(|(name, (path, _))| (*name, (path.clone(), after[name].clone()))).collect()
}

/// The settings in `bytes`, read as the agent's format reads them.
fn settings(format: Format, bytes: &[u8], agent: &Agent) -> serde_json::Value {
    let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{}: not text after connecting", agent.name));
    match format {
        Format::Json => serde_json::from_str(text).unwrap_or_else(|error| panic!("{}: not JSON: {error}", agent.name)),
        Format::Jsonc => jsonc_parser::parse_to_serde_value(text, &Default::default())
            .unwrap_or_else(|error| panic!("{}: not JSONC: {error}", agent.name)),
        Format::Yaml => {
            serde_yaml_ng::from_str(text).unwrap_or_else(|error| panic!("{}: not YAML: {error}", agent.name))
        }
    }
}

#[test]
fn every_agent_connects_and_is_taken_out_whole() {
    let command = if cfg!(windows) { r"C:\Users\me\.fastctx\bin\fastctx.exe" } else { "/home/me/.fastctx/bin/fastctx" };
    let args = vec!["serve".to_owned()];
    let env = vec![("FASTCTX_HOME".to_owned(), "/home/me/.fastctx".to_owned())];
    let server = Server { command, args: &args, env: &env };
    let body = fastctx_install::briefing::guidance(false);
    for agent in agents::all() {
        let home = fixture(agent).join("home");
        assert!(home.is_dir(), "{}: no sample home at {}", agent.name, home.display());
        let before = files_of(agent, &home);

        let connected = agents::connect(agent, &before, None, &server, &body, false)
            .unwrap_or_else(|error| panic!("{}: {error}", agent.name));
        let file = connected.receipt.server_file().expect("the server is placed");
        let place = agent.mcp.to.places().iter().find(|place| place.file == file).expect("a place of the agent's");
        let written = connected.files[file].as_ref().expect("the server's file is written");
        let held = place.at.iter().fold(settings(place.format, written, agent), |value, key| value[*key].clone());
        let entry = (agent.mcp.entry)(&server);
        match agent.mcp.slot {
            Slot::Named => assert_eq!(held["fastctx"], entry, "{}: the server's entry", agent.name),
            Slot::Listed { .. } => assert!(
                held.as_array().is_some_and(|servers| servers.contains(&entry)),
                "{}: the server's entry",
                agent.name
            ),
        }
        assert!(!connected.steps.cards().is_empty(), "{}: connecting tells no change", agent.name);

        let after = with(&before, &connected.files);
        let again = agents::connect(agent, &after, Some(&connected.receipt), &server, &body, false)
            .unwrap_or_else(|error| panic!("{}: connecting again: {error}", agent.name));
        assert_eq!(again.files, connected.files, "{}: connecting again changes files", agent.name);
        assert!(
            !agents::taken_out(agent, &after, &connected.receipt).expect("read"),
            "{}: what was just written is missed",
            agent.name
        );

        let taken = agents::disconnect(agent, &after, &connected.receipt)
            .unwrap_or_else(|error| panic!("{}: taking out: {error}", agent.name));
        for (name, (_, bytes)) in &before {
            assert_eq!(&taken.files[name], bytes, "{}: {name} is not given back as it was", agent.name);
        }
        assert!(taken.notices.is_empty(), "{}: taking out left places: {:?}", agent.name, taken.notices);

        let expected = fixture(agent).join("identity.json");
        if expected.exists() {
            let expected: Identity = serde_json::from_slice(&std::fs::read(&expected).expect("the identity is read"))
                .expect("the identity parses");
            assert_eq!((agent.identity)(&home), Some(expected), "{}: who is logged in", agent.name);
        }
    }
}

#[test]
fn a_user_change_to_fastctx_s_entry_is_kept() {
    let args = vec!["serve".to_owned()];
    let server = Server { command: "/fastctx", args: &args, env: &[] };
    let body = fastctx_install::briefing::guidance(false);
    for agent in agents::all() {
        let home = fixture(agent).join("home");
        let before = files_of(agent, &home);
        let connected = agents::connect(agent, &before, None, &server, &body, false).expect("connects");
        // The user points Fastctx's entry somewhere else.
        let file = connected.receipt.server_file().expect("the server is placed");
        let text = String::from_utf8(connected.files[file].clone().expect("written")).expect("text");
        let changed = text.replacen("/fastctx", "/elsewhere/fastctx", 1);
        let mut after = with(&before, &connected.files);
        after.get_mut(file).expect("read").1 = Some(changed.clone().into_bytes());
        assert!(
            agents::connect(agent, &after, Some(&connected.receipt), &server, &body, false).is_err(),
            "{}: a changed entry is written over",
            agent.name
        );
        let taken = agents::disconnect(agent, &after, &connected.receipt).expect("takes out");
        assert!(
            String::from_utf8_lossy(taken.files[file].as_deref().unwrap_or_default()).contains("/elsewhere/fastctx"),
            "{}: the user's entry is taken out",
            agent.name
        );
        assert!(!taken.notices.is_empty(), "{}: the kept entry is not told", agent.name);
    }
}
