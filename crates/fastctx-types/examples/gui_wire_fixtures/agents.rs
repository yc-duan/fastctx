use fastctx_types::agents::*;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Call, Reply};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub calls: Vec<Call>,
    pub replies: Vec<Reply>,
    pub remote_calls: Vec<RemoteCall>,
    pub remote_replies: Vec<RemoteReply>,
}

pub fn fixtures() -> Fixtures {
    let machine: fastctx_types::MachineName = "desktop".parse().unwrap();
    let checked_at = "2026-10-04T12:06:07Z".parse().unwrap();
    let codex = AgentFacts {
        agent: "codex".parse().unwrap(),
        program: Fact::known(Some(AgentProgram {
            path: "C:/app/codex.exe".into(),
            source: ProgramSource::DesktopApp,
            exists: true,
        })),
        version: Fact::known("codex-cli fixture-version".into()),
        launch: Fact::known(LaunchCheck::CodexAppServer { program: "C:/app/codex.exe".into(), pid: 123 }),
        login: Fact::known(LoginStatus::LoggedIn { method: Some("chatgpt".into()) }),
        models: Fact::known(vec![AgentModel {
            id: "model-a".into(),
            model: "model-a".into(),
            display_name: "Example model".into(),
            description: "Catalog fixture".into(),
            is_default: true,
            default_reasoning_effort: "medium".into(),
            reasoning_efforts: vec![ReasoningEffort { effort: "medium".into(), description: "Example effort".into() }],
        }]),
        integration: Fact::known(Integration::In),
        unattended: Fact::known(true),
        timings: vec![ProbeTime { step: "model/list".into(), elapsed_ms: 0 }],
    };
    let claude = AgentFacts {
        agent: "claude".parse().unwrap(),
        program: Fact::known(Some(AgentProgram {
            path: "/usr/bin/claude".into(),
            source: ProgramSource::Path,
            exists: true,
        })),
        version: Fact::known("fixture-version (Claude Code)".into()),
        launch: Fact::known(LaunchCheck::ClaudeCommand),
        login: Fact::known(LoginStatus::LoggedOut),
        models: Fact::unavailable("no public non-session CLI catalog"),
        integration: Fact::known(Integration::Out),
        unattended: Fact::known(false),
        timings: vec![],
    };
    let mut missing = codex.clone();
    missing.program = Fact::known(None);
    missing.version = Fact::unavailable("not installed");
    missing.launch = Fact::unavailable("not installed");
    missing.login = Fact::unavailable("not installed");
    missing.models = Fact::unavailable("not installed");
    missing.integration = Fact::known(Integration::Absent);
    let mut broken = codex.clone();
    broken.program = Fact::known(Some(AgentProgram {
        path: "C:/old/codex.exe".into(),
        source: ProgramSource::DesktopApp,
        exists: false,
    }));
    broken.version = Fact::failed("program disappeared");
    broken.launch = Fact::failed("cannot spawn");
    broken.login = Fact::failed("status command failed");
    broken.models = Fact::failed("model/list failed");
    broken.integration = Fact::known(Integration::Old);
    let mut unknown = claude.clone();
    unknown.program = Fact::failed("configuration unreadable");
    unknown.integration = Fact::failed("receipt unreadable");
    unknown.login = Fact::known(LoginStatus::LoggedIn { method: None });
    unknown.models = Fact::known(vec![]);
    let answer = AgentsReply {
        machine: machine.clone(),
        checked_at,
        elapsed_ms: 420,
        agents: vec![codex, claude, missing, broken, unknown],
    };
    Fixtures {
        calls: vec![
            Call::Agents { machine: None },
            Call::Agents { machine: Some(machine.clone()) },
            Call::Open {
                agent: "codex".parse().unwrap(),
                town: None,
                node: Some(machine.clone()),
                text: "First words".into(),
                model: Some("model-a".into()),
                reasoning_effort: Some("medium".into()),
                attachments: Vec::new(),
            },
            Call::Open {
                agent: "claude".parse().unwrap(),
                town: None,
                node: None,
                text: "Defaults".into(),
                model: None,
                reasoning_effort: None,
                attachments: Vec::new(),
            },
        ],
        replies: vec![Reply::Agents(WorldAgents {
            machine: machine.clone(),
            machines: vec![
                MachineAgents::Answered { reply: answer.clone() },
                MachineAgents::Offline { machine: "laptop".parse().unwrap(), since: Some(checked_at) },
                MachineAgents::Offline { machine: "unknown".parse().unwrap(), since: None },
                MachineAgents::NoAnswer { machine: "lab".parse().unwrap(), reason: "connection broke".into() },
            ],
        })],
        remote_calls: vec![
            RemoteCall::Agents,
            RemoteCall::Open {
                agent: "claude".parse().unwrap(),
                town: None,
                text: "First words".into(),
                model: Some("opus".into()),
                reasoning_effort: Some("high".into()),
                attachments: Vec::new(),
            },
        ],
        remote_replies: vec![RemoteReply::Agents(answer)],
    }
}
