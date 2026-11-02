//! Every agent connected through its configuration alone, against a home
//! its own program wrote (`tests/fixtures/agents/<name>/`): connecting writes
//! files its format still reads, with Fastctx's server where the agent
//! looks for it; connecting again changes nothing; taking it out gives back
//! every file byte for byte; and its files name who is logged in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fastctx_install::agents::{self, Agent, Files, Format, Server};
use fastctx_types::logins::Identity;

fn fixture(agent: &Agent) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents").join(agent.name)
}

fn files_of(agent: &Agent, home: &Path) -> Files {
    agent
        .files()
        .into_iter()
        .map(|name| {
            let path = home.join(name);
            (name, (path.display().to_string(), std::fs::read(&path).ok()))
        })
        .collect()
}

fn with(files: &Files, after: &BTreeMap<&'static str, Option<Vec<u8>>>) -> Files {
    files.iter().map(|(name, (path, _))| (*name, (path.clone(), after[name].clone()))).collect()
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
        let mcp = connected.files[agent.mcp.file].as_ref().expect("the server's file is written");
        if agent.mcp.format == Format::Json {
            let settings: serde_json::Value = serde_json::from_slice(mcp)
                .unwrap_or_else(|error| panic!("{}: not JSON after connecting: {error}", agent.name));
            let mut place = &settings;
            for key in agent.mcp.at {
                place = &place[*key];
            }
            assert_eq!(place["fastctx"], (agent.mcp.entry)(&server), "{}: the server's entry", agent.name);
        }
        assert!(!connected.steps.cards().is_empty(), "{}: connecting tells no change", agent.name);

        let after = with(&before, &connected.files);
        let again = agents::connect(agent, &after, Some(&connected.receipt), &server, &body, false)
            .unwrap_or_else(|error| panic!("{}: connecting again: {error}", agent.name));
        assert_eq!(again.files, connected.files, "{}: connecting again changes files", agent.name);

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
        let file = agent.mcp.file;
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
