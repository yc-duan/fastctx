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
    let reset = |at: &str| Some(at.parse().unwrap());
    let codex = AgentFacts {
        agent: "codex".parse().unwrap(),
        home: Some(AgentHome { dir: "C:/Users/ada/.codex".into(), found: HomeFound::Default }),
        account: Fact::known(AgentAccount {
            email: Some("ada@example.com".into()),
            organization: None,
            plan: Some("plus".into()),
        }),
        limits: Fact::known(AgentLimits {
            observed_at: checked_at,
            windows: vec![
                LimitWindow {
                    name: "codex".into(),
                    minutes: Some(300),
                    used_tenths: 423,
                    resets_at: reset("2026-10-04T14:00:00Z"),
                    since: None,
                },
                LimitWindow {
                    name: "codex".into(),
                    minutes: Some(10080),
                    used_tenths: 1000,
                    resets_at: reset("2026-10-09T00:00:00Z"),
                    since: None,
                },
                LimitWindow { name: "codex_other".into(), minutes: None, used_tenths: 0, resets_at: None, since: None },
            ],
            credits: Some(AgentCredits { balance: Some("52230.1460550000".into()), unlimited: false }),
        }),
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
            context_window: Some(272_000),
            max_context_window: Some(872_000),
        }]),
        integration: Fact::known(Integration::In),
        unattended: Fact::known(true),
        timings: vec![ProbeTime { step: "model/list".into(), elapsed_ms: 0 }],
        drift: None,
    };
    let claude = AgentFacts {
        agent: "claude".parse().unwrap(),
        home: Some(AgentHome { dir: "/home/ada/.claude-work".into(), found: HomeFound::Found }),
        account: Fact::known(AgentAccount {
            email: Some("ada@example.org".into()),
            organization: Some("Example Org".into()),
            plan: Some("max".into()),
        }),
        limits: Fact::known(AgentLimits {
            observed_at: checked_at,
            windows: vec![LimitWindow {
                name: "five_hour".into(),
                minutes: Some(300),
                used_tenths: 1013,
                resets_at: reset("2026-10-04T15:00:00Z"),
                since: None,
            }],
            credits: None,
        }),
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
        drift: None,
    };
    let mut missing = codex.clone();
    // What a machine that knew one home per agent answers.
    missing.home = None;
    missing.account = Fact::unavailable("this machine's World does not report it");
    missing.limits = Fact::unavailable("this machine's World does not report it");
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
    unknown.home = Some(AgentHome { dir: "D:/agents/claude-token".into(), found: HomeFound::Added });
    // A long-lived token names no one.
    unknown.account = Fact::known(AgentAccount::default());
    unknown.limits = Fact::unavailable("no session in this home has reported its limits yet");
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
            Call::Agents { machine: Some(machine.clone().into()) },
            Call::Open {
                agent: "codex".parse().unwrap(),
                town: None,
                node: Some(machine.clone().into()),
                text: "First words".into(),
                model: Some("model-a".into()),
                reasoning_effort: Some("medium".into()),
                attachments: Vec::new(),
                choices: fastctx_types::agents::Choices {
                    mcp_off: Default::default(),
                    account: Some(fastctx_types::access::HubAccount {
                        hub: "w-0123456789abcdef".parse().unwrap(),
                        account: "ada".parse().unwrap(),
                    }),
                    focus: fastctx_types::focus::Focus {
                        machines: vec![
                            fastctx_types::focus::FocusedMachine {
                                machine: "lab.home".parse().unwrap(),
                                folders: vec!["C:/work/中文 项目".into(), "D:/data".into()],
                            },
                            fastctx_types::focus::FocusedMachine {
                                machine: "laptop".parse().unwrap(),
                                folders: vec![],
                            },
                        ],
                    },
                    home: Some("C:/Users/ada/.codex-work".into()),
                    files_on: None,
                },
            },
            Call::Open {
                agent: "claude".parse().unwrap(),
                town: None,
                node: None,
                text: "Defaults".into(),
                model: None,
                reasoning_effort: None,
                attachments: Vec::new(),
                choices: Default::default(),
            },
            Call::ClaudeConsent { machine: machine.clone().into(), accept: true, home: None },
            Call::ClaudeConsent {
                machine: machine.clone().into(),
                accept: false,
                home: Some("/home/ada/.claude-work".into()),
            },
            Call::Homes {
                machine: machine.clone().into(),
                change: HomeChange::Add { agent: "claude".parse().unwrap(), dir: "D:/agents/claude-token".into() },
            },
            Call::Homes {
                machine: machine.clone().into(),
                change: HomeChange::Hide { agent: "codex".parse().unwrap(), dir: "C:/Users/ada/.codex-old".into() },
            },
            Call::Homes {
                machine: machine.clone().into(),
                change: HomeChange::Show { agent: "codex".parse().unwrap(), dir: "C:/Users/ada/.codex-old".into() },
            },
        ],
        replies: vec![Reply::Agents(WorldAgents {
            machine: machine.clone(),
            machines: vec![
                MachineAgents::Answered { observation: None, reply: answer.clone() },
                MachineAgents::Offline {
                    observation: None,
                    machine: "laptop".parse().unwrap(),
                    since: Some(checked_at),
                },
                MachineAgents::Offline { observation: None, machine: "unknown".parse().unwrap(), since: None },
                MachineAgents::NoAnswer {
                    observation: None,
                    machine: "lab".parse().unwrap(),
                    reason: "connection broke".into(),
                },
            ],
        })],
        remote_calls: vec![
            RemoteCall::Agents,
            RemoteCall::Open {
                mcp_off: Default::default(),
                agent: "claude".parse().unwrap(),
                town: None,
                text: "First words".into(),
                model: Some("opus".into()),
                reasoning_effort: Some("high".into()),
                attachments: Vec::new(),
                focus: fastctx_types::focus::Focus {
                    machines: vec![
                        fastctx_types::focus::FocusedMachine {
                            machine: "lab.home".parse().unwrap(),
                            folders: vec!["C:/work/中文 项目".into(), "D:/data".into()],
                        },
                        fastctx_types::focus::FocusedMachine { machine: "laptop".parse().unwrap(), folders: vec![] },
                    ],
                },
                home: Some("/home/ada/.claude-work".into()),
                files_on: Some("laptop".parse().unwrap()),
            },
            RemoteCall::ClaudeConsent { accept: true, home: Some("/home/ada/.claude-work".into()) },
            RemoteCall::Homes {
                change: HomeChange::Add { agent: "codex".parse().unwrap(), dir: "/srv/codex-team".into() },
            },
        ],
        remote_replies: vec![RemoteReply::Agents(answer), RemoteReply::Set],
    }
}
