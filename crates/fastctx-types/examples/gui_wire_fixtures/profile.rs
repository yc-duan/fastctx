use fastctx_types::{Call, Reply, mcp::*, profile::*};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    replies: Vec<Reply>,
}

pub fn fixtures() -> Fixtures {
    let remote = RemoteMcp {
        url: "https://mcp.example.org/tools".into(),
        sse: false,
        headers: BTreeMap::from([("Authorization".into(), "Bearer fixture".into())]),
    };
    let command = CommandMcp {
        command: "python".into(),
        args: vec!["-m".into(), "example_mcp".into()],
        env: BTreeMap::from([("PYTHONUTF8".into(), "1".into())]),
        cwd: None,
    };
    let profile = Profile {
        version: 4,
        memory: false,
        subagents: None,
        prompt: "Use concise answers.".into(),
        claude: serde_json::json!({"language":"English"}).as_object().unwrap().clone(),
        codex: serde_json::json!({"model_verbosity":"low"}).as_object().unwrap().clone(),
        servers: BTreeMap::from([
            ("docs".into(), Entry::Remote(remote.clone())),
            ("editor".into(), Entry::Command { machine: "laptop".parse().unwrap() }),
        ]),
    };
    let calls = vec![
        Call::ProfileRead { hub: None, account: None },
        Call::ProfileWrite {
            hub: Some("home".parse().unwrap()),
            account: Some("ada".parse().unwrap()),
            profile: profile.clone(),
        },
        Call::Mcp {
            hub: None,
            account: None,
            mcp: McpCall::Keep {
                name: "editor".into(),
                machine: Some("laptop".parse().unwrap()),
                server: Server::Command(command),
            },
        },
    ];
    let listed = vec![
        Listed { name: "docs".into(), server: ListedServer::Remote(remote) },
        Listed {
            name: "editor".into(),
            server: ListedServer::Command { machine: "laptop".parse().unwrap(), state: CommandState::Kept },
        },
    ];
    let replies = vec![
        Reply::Profile(Box::new(ProfileView {
            hub: "home".parse().unwrap(),
            account: "ada".parse().unwrap(),
            profile,
            kept: true,
        })),
        Reply::ProfileWritten { version: 5 },
        Reply::Mcp { result: McpReply::Listed { servers: listed } },
        Reply::Mcp { result: McpReply::Tested(Tested { tools: vec!["scene".into()], problem: None, log: None }) },
        Reply::Mcp {
            result: McpReply::Found {
                machine: "laptop".parse().unwrap(),
                servers: vec![Found { source: Source::Cursor, name: "editor".into(), server: FoundServer::Command }],
            },
        },
    ];
    Fixtures { calls, replies }
}
