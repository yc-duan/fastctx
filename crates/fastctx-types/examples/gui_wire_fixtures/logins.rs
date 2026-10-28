use fastctx_types::agents::*;
use fastctx_types::logins::*;
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
    let at: jiff::Timestamp = "2026-10-11T12:00:00Z".parse().unwrap();
    let codex: fastctx_types::AgentName = "codex".parse().unwrap();
    let claude: fastctx_types::AgentName = "claude".parse().unwrap();
    let listed = Login {
        dir: "C:/Users/me/.codex".into(),
        default: true,
        found: Found::Default,
        identity: Some(Identified {
            at,
            who: Fact::known(Identity {
                logged_in: true,
                method: Some("chatgpt".into()),
                email: Some("me@example.com".into()),
                organization: None,
                plan: Some("pro".into()),
            }),
        }),
        limits: Fact::known(AgentLimits {
            observed_at: at,
            windows: vec![LimitWindow {
                name: "codex".into(),
                minutes: Some(10080),
                used_tenths: 1000,
                resets_at: Some("2026-10-14T00:00:00Z".parse().unwrap()),
                since: None,
            }],
            credits: Some(AgentCredits { balance: Some("52086".into()), unlimited: false }),
        }),
        programs: vec![0, 1],
        integration: Integration::In,
        unattended: None,
        auto_title: true,
        launcher: Some(Launcher {
            path: "C:/app/codex.exe".into(),
            version: Some("0.162.0".into()),
            desktop_app: true,
        }),
        models: vec![AgentModel {
            id: "gpt-6-luna".into(),
            model: "gpt-6-luna".into(),
            display_name: "GPT-6 Luna".into(),
            description: String::new(),
            is_default: true,
            default_reasoning_effort: "low".into(),
            reasoning_efforts: vec![ReasoningEffort { effort: "low".into(), description: String::new() }],
            context_window: None,
            max_context_window: None,
        }],
        change: Some(LoginChange {
            into: true,
            started_at: at,
            steps: vec![
                ChangeStep { step: Step::Program, state: StepState::Done },
                ChangeStep {
                    step: Step::Files { changed: vec!["C:/Users/me/.codex/config.toml".into()] },
                    state: StepState::Done,
                },
                ChangeStep {
                    step: Step::Check { scope: "effective Codex features".into() },
                    state: StepState::Failed { reason: "still enabled: shell_tool".into() },
                },
            ],
            end: Some(ChangeEnd::Done {
                at,
                kept: vec![Kept { location: "AGENTS.md".into(), reason: "changed since apply; left untouched".into() }],
            }),
        }),
        drift: Some(Drift::Changed {
            location: "C:/Users/me/.codex/AGENTS.md".into(),
            reason: "the fastctx block differs from the recorded bytes".into(),
        }),
    };
    let candidate = Login {
        dir: "/home/me/.claude-work".into(),
        default: false,
        found: Found::Home,
        identity: Some(Identified {
            at,
            who: Fact::known(Identity { logged_in: true, method: Some("oauth_token".into()), ..Identity::default() }),
        }),
        limits: Fact::unavailable("no session in this login has reported its limits yet"),
        programs: vec![],
        integration: Integration::Out,
        unattended: Some(false),
        auto_title: false,
        launcher: Some(Launcher { path: "/home/me/.local/bin/claude".into(), version: None, desktop_app: false }),
        models: vec![],
        change: Some(LoginChange {
            into: true,
            started_at: at,
            steps: vec![ChangeStep {
                step: Step::Command { command: "claude plugin install fastctx-world@fastctx --scope user".into() },
                state: StepState::Running,
            }],
            end: None,
        }),
        drift: None,
    };
    let never = Login {
        dir: "C:/Users/me/CodexHomes/b".into(),
        default: false,
        found: Found::Elsewhere,
        identity: None,
        limits: Fact::failed("account/rateLimits/read: refused"),
        programs: vec![],
        integration: Integration::Old,
        unattended: None,
        auto_title: false,
        launcher: None,
        models: vec![],
        change: Some(LoginChange {
            into: true,
            started_at: at,
            steps: vec![],
            end: Some(ChangeEnd::Failed {
                at,
                reason: "the limit is lower".into(),
                output_limit: Some(OutputLimit { location: "config.toml".into(), current: 1000, requested: 26000 }),
            }),
        }),
        drift: Some(Drift::Asks {
            limit: OutputLimit { location: "config.toml".into(), current: 1000, requested: 26000 },
        }),
    };
    let record = MachineLogins {
        machine: machine.clone(),
        scan: Some(Scan {
            started_at: at,
            usual: Stage::Done { at },
            unreadable: vec!["C:/Program Files/WindowsApps: access denied".into()],
        }),
        codex: AgentLogins {
            programs: vec![
                FoundProgram {
                    path: Some("C:/app/codex.exe".into()),
                    how: Installed::DesktopApp,
                    version: None,
                    within: Some(Within {
                        name: "ChatGPT".into(),
                        id: "OpenAI.Codex".into(),
                        version: Some("26.1007.2314.0".into()),
                        path: "C:/Program Files/WindowsApps/OpenAI.Codex".into(),
                    }),
                    on_path: false,
                },
                FoundProgram {
                    path: Some("C:/npm/codex.cmd".into()),
                    how: Installed::Npm,
                    version: Some("0.162.1".into()),
                    within: None,
                    on_path: true,
                },
            ],
            listed: vec![listed],
            candidates: vec![never],
            elsewhere: vec![],
        },
        claude: AgentLogins {
            programs: vec![FoundProgram {
                path: None,
                how: Installed::DesktopApp,
                version: None,
                within: Some(Within {
                    name: "Claude".into(),
                    id: "Claude".into(),
                    version: None,
                    path: "C:/apps/Claude".into(),
                }),
                on_path: false,
            }],
            listed: vec![],
            candidates: vec![candidate],
            elsewhere: vec![],
        },
        folders: vec![
            Folder {
                dir: "C:/Users/me/CodexHomes".into(),
                search: Some(FolderSearch {
                    stage: Stage::Done { at },
                    directories: 1204,
                    unreadable: 0,
                    found: vec![FoundIn {
                        agent: codex.clone(),
                        path: "work".into(),
                        traces: vec!["auth.json".into(), "sessions".into()],
                    }],
                }),
            },
            Folder {
                dir: "D:/".into(),
                search: Some(FolderSearch { stage: Stage::Running, directories: 12345, unreadable: 2, found: vec![] }),
            },
            Folder {
                dir: "E:/old".into(),
                search: Some(FolderSearch {
                    stage: Stage::Stopped { at },
                    directories: 80,
                    unreadable: 0,
                    found: vec![FoundIn {
                        agent: claude.clone(),
                        path: "profiles/.claude-team".into(),
                        traces: vec![".credentials.json".into()],
                    }],
                }),
            },
            Folder { dir: "F:/never".into(), search: None },
        ],
    };
    let empty = MachineLogins {
        machine: "laptop".parse().unwrap(),
        scan: Some(Scan {
            started_at: at,
            usual: Stage::Failed { at, reason: "the node stopped during it".into() },
            unreadable: vec![],
        }),
        codex: AgentLogins::default(),
        claude: AgentLogins::default(),
        folders: vec![],
    };
    let preview = ChangePreview {
        agent: claude.clone(),
        dir: "/home/me/.claude-work".into(),
        into: true,
        files: vec![
            FilePreview {
                path: "/home/me/.claude-work/settings.json".into(),
                how: FileHow::Changed,
                items: vec![
                    ItemChange {
                        item: "env.MAX_MCP_OUTPUT_TOKENS".into(),
                        how: FileHow::Created,
                        before: None,
                        after: Some("\"26000\"".into()),
                        withheld: false,
                    },
                    ItemChange {
                        item: "skipDangerousModePermissionPrompt".into(),
                        how: FileHow::Created,
                        before: None,
                        after: Some("true".into()),
                        withheld: false,
                    },
                    ItemChange {
                        item: "env.SOMETHING".into(),
                        how: FileHow::Changed,
                        before: None,
                        after: None,
                        withheld: true,
                    },
                ],
            },
            FilePreview {
                path: "/home/me/.claude-work/CLAUDE.md".into(),
                how: FileHow::Created,
                items: vec![ItemChange {
                    item: "fastctx".into(),
                    how: FileHow::Created,
                    before: None,
                    after: Some("<!-- fastctx:begin -->".into()),
                    withheld: false,
                }],
            },
        ],
        commands: vec!["claude plugin marketplace add /home/me/.fastctx/world/claude-marketplace --scope user".into()],
        output_limit: None,
        kept: vec![],
    };
    let codex_preview = ChangePreview {
        agent: codex.clone(),
        dir: "C:/Users/me/.codex".into(),
        into: false,
        files: vec![FilePreview { path: "C:/Users/me/.codex/AGENTS.md".into(), how: FileHow::Removed, items: vec![] }],
        commands: vec![],
        output_limit: Some(OutputLimit {
            location: "config.toml: tool_output_token_limit".into(),
            current: 1000,
            requested: 26000,
        }),
        kept: vec![Kept {
            location: "config.toml: features.shell_tool".into(),
            reason: "changed since apply; left untouched".into(),
        }],
    };
    let change = LoginChange {
        into: false,
        started_at: at,
        steps: vec![ChangeStep { step: Step::Files { changed: vec![] }, state: StepState::Done }],
        end: Some(ChangeEnd::Done { at, kept: vec![] }),
    };
    Fixtures {
        calls: vec![
            Call::AgentLogins { machine: None },
            Call::ScanAgents { machine: machine.clone().into(), stop: false },
            Call::AddAgentLogin {
                machine: machine.clone().into(),
                agent: codex.clone(),
                dir: "D:/backup/.codex".into(),
            },
            Call::AgentFolder {
                machine: machine.clone().into(),
                dir: "C:/Users/me/CodexHomes".into(),
                change: FolderChange::Choose,
            },
            Call::AgentFolder { machine: machine.clone().into(), dir: "D:/".into(), change: FolderChange::Stop },
            Call::AgentFolder { machine: machine.clone().into(), dir: "E:/old".into(), change: FolderChange::Forget },
            Call::PreviewAgentChange {
                machine: machine.clone().into(),
                agent: claude.clone(),
                dir: "/home/me/.claude-work".into(),
                into: true,
                take_back: true,
            },
            Call::ChangeAgentLogin {
                machine: machine.clone().into(),
                agent: codex.clone(),
                dir: "C:/Users/me/.codex".into(),
                into: true,
                raise_output_limit: true,
                take_back: true,
            },
            Call::ReadAgentLimits {
                machine: machine.clone().into(),
                agent: codex.clone(),
                dir: "C:/Users/me/.codex".into(),
            },
        ],
        replies: vec![
            Reply::AgentLogins(WorldLogins {
                machine: machine.clone(),
                machines: vec![
                    MachineLoginsHeard::Answered { observation: None, logins: Box::new(record.clone()) },
                    MachineLoginsHeard::Answered { observation: None, logins: Box::new(empty) },
                    MachineLoginsHeard::Offline { observation: None, machine: "gpu".parse().unwrap(), since: Some(at) },
                    MachineLoginsHeard::NoAnswer {
                        observation: None,
                        machine: "old".parse().unwrap(),
                        reason: "it answered something else".into(),
                    },
                ],
            }),
            Reply::MachineAgentLogins(Box::new(record.clone())),
            Reply::AgentChangePreview(Box::new(preview.clone())),
            Reply::AgentChangePreview(Box::new(codex_preview)),
            Reply::AgentLoginChanged(Box::new(change.clone())),
        ],
        remote_calls: vec![
            RemoteCall::AgentLogins,
            RemoteCall::ScanAgents { stop: true },
            RemoteCall::AddAgentLogin { agent: claude.clone(), dir: "/home/me/.claude-work".into() },
            RemoteCall::AgentFolder { dir: "/home/me/agents".into(), change: FolderChange::Choose },
            RemoteCall::PreviewAgentChange {
                agent: claude.clone(),
                dir: "/home/me/.claude-work".into(),
                into: false,
                take_back: false,
            },
            RemoteCall::ChangeAgentLogin {
                agent: claude,
                dir: "/home/me/.claude-work".into(),
                into: false,
                raise_output_limit: false,
                take_back: false,
            },
            RemoteCall::ReadAgentLimits { agent: codex, dir: "C:/Users/me/.codex".into() },
        ],
        remote_replies: vec![
            RemoteReply::AgentLogins(Box::new(record)),
            RemoteReply::AgentChangePreview(Box::new(preview)),
            RemoteReply::AgentLoginChanged(Box::new(change)),
        ],
    }
}
