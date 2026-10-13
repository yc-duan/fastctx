use fastctx_types::access::{HubAccount, Written};
use fastctx_types::accounts::*;
use fastctx_types::agents::Choices;
use fastctx_types::device::{ConnectionMode, ConnectionState, DeviceHub, DeviceState};
use fastctx_types::world::{LinkState, Route};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub replies: Vec<AccountsReply>,
    pub changes: Vec<AccountChange>,
    pub connections: Vec<ConnectionState>,
    pub choices: Vec<Choices>,
}

fn at() -> Timestamp {
    "2026-10-09T08:30:00Z".parse().unwrap()
}

fn online() -> LinkState {
    LinkState::Online {
        since: at(),
        hub: "203.0.113.20:7444".into(),
        through: Some("server".into()),
        route: Route::Local,
        ms: Some(38),
    }
}

fn hub() -> HubAccounts {
    HubAccounts {
        hub: "w-0123456789abcdef".parse().unwrap(),
        name: "home".parse().unwrap(),
        addresses: vec!["203.0.113.10:7443".parse().unwrap()],
        link: online(),
        me: Some("desktop".parse().unwrap()),
        renames: true,
        logins: vec!["ada".parse().unwrap()],
        home: Some("ada".parse().unwrap()),
        self_registration: true,
        accounts: vec![
            AccountSeen { name: "ada".parse().unwrap(), level: Level::Super, locked: false, opened: at() },
            AccountSeen { name: "bo".parse().unwrap(), level: Level::Member, locked: false, opened: at() },
        ],
        members: vec![
            MemberSeen {
                name: "desktop".parse().unwrap(),
                was: vec![],
                kind: MemberKind::Computer,
                logins: vec!["ada".parse().unwrap()],
                home: Some("ada".parse().unwrap()),
                direct: vec![],
                added: at(),
                online: true,
                since: Some(at()),
                through: Some("server".into()),
                reach: Some(Reach {
                    seen: None,
                    interfaces: vec!["192.168.1.20".parse().unwrap(), "fd00::20".parse().unwrap()],
                }),
                paths: Some(Paths {
                    hub_ms: Some(38),
                    links: vec![LinkSeen {
                        peer: "server".parse().unwrap(),
                        up: true,
                        ms: Some(41),
                        since: at(),
                        why: None,
                    }],
                }),
                key: "5be1fcee".into(),
            },
            MemberSeen {
                name: "server".parse().unwrap(),
                was: vec!["izbp1abc0x2y3z".parse().unwrap()],
                kind: MemberKind::Computer,
                logins: vec!["ada".parse().unwrap()],
                home: Some("ada".parse().unwrap()),
                direct: vec!["203.0.113.20:7444".parse().unwrap()],
                added: at(),
                online: true,
                since: Some(at()),
                through: None,
                reach: Some(Reach { seen: Some("203.0.113.20".parse().unwrap()), interfaces: vec![] }),
                paths: Some(Paths {
                    hub_ms: Some(160),
                    links: vec![LinkSeen {
                        peer: "laptop".parse().unwrap(),
                        up: false,
                        ms: None,
                        since: at(),
                        why: Some("203.0.113.20:7444: no answer within 30 s".into()),
                    }],
                }),
                key: "71c0de55".into(),
            },
            MemberSeen {
                name: "pixel".parse().unwrap(),
                was: vec![],
                kind: MemberKind::Phone,
                logins: vec!["ada".parse().unwrap()],
                home: None,
                direct: vec![],
                added: at(),
                online: false,
                since: None,
                through: None,
                reach: None,
                paths: None,
                key: "4f2a91aa".into(),
            },
        ],
        shares: vec![ShareSeen { machine: "desktop".parse().unwrap(), account: "bo".parse().unwrap() }],
        assists: vec![
            AssistSeen {
                machine: "desktop".parse().unwrap(),
                account: "cy".parse().unwrap(),
                agent: "codex".parse().unwrap(),
                home: None,
                until: None,
            },
            AssistSeen {
                machine: "gpu".parse().unwrap(),
                account: "ada".parse().unwrap(),
                agent: "claude".parse().unwrap(),
                home: Some("/home/bo/.claude-team".into()),
                until: Some("2026-10-20T10:00:00Z".parse().unwrap()),
            },
        ],
        plans: vec![PlanSeen {
            machine: "gpu".parse().unwrap(),
            agent: "claude".parse().unwrap(),
            home: Some("/home/bo/.claude-team".into()),
            plan: PlanLabel::Max,
        }],
        acting: vec![ActingSeen { machine: "gpu".parse().unwrap(), account: "ada".parse().unwrap(), since: at() }],
        projects: vec![ProjectSeen {
            name: "mark50".parse().unwrap(),
            owner: "ada".parse().unwrap(),
            granted: vec!["bo".parse().unwrap()],
        }],
        arrivals: vec![Arrival {
            hub: "w-0123456789abcdef".parse().unwrap(),
            account: "ada".parse().unwrap(),
            name: "pixel".parse().unwrap(),
            device: true,
            noticed_at: at(),
        }],
        links: vec![LinkSeen { peer: "server".parse().unwrap(), up: true, ms: Some(41), since: at(), why: None }],
    }
}

pub fn fixtures() -> Fixtures {
    let machine = || "desktop".parse().unwrap();
    let account = || "ada".parse().unwrap();
    Fixtures {
        replies: vec![
            AccountsReply { machine: machine(), was: vec![], hubs: vec![hub()] },
            AccountsReply { machine: machine(), was: vec!["desktop-7q2m9x".parse().unwrap()], hubs: vec![] },
        ],
        changes: vec![
            AccountChange::Share { machine: machine(), to: "bo".parse().unwrap(), shared: true },
            AccountChange::Assist {
                machine: machine(),
                to: "cy".parse().unwrap(),
                agent: "codex".parse().unwrap(),
                home: Some("C:/Users/ada/.codex-work".into()),
                assisting: true,
                until: Some("2026-10-20T10:00:00Z".parse().unwrap()),
            },
            AccountChange::Plan {
                machine: "gpu".parse().unwrap(),
                agent: "claude".parse().unwrap(),
                home: None,
                plan: Some(PlanLabel::Pro),
            },
            AccountChange::Plan {
                machine: "gpu".parse().unwrap(),
                agent: "claude".parse().unwrap(),
                home: Some("/home/bo/.claude-team".into()),
                plan: None,
            },
            AccountChange::StopActing { machine: "gpu".parse().unwrap() },
            AccountChange::Direct {
                machine: "server".parse().unwrap(),
                at: vec!["203.0.113.20:7444".parse().unwrap()],
            },
            AccountChange::Direct { machine: "server".parse().unwrap(), at: vec![] },
            AccountChange::Remove { name: "pixel".parse().unwrap(), reason: "lost".into() },
            AccountChange::Rename { machine: "server".parse().unwrap(), to: "台式机".parse().unwrap() },
            AccountChange::Logout,
            AccountChange::Token {
                register: true,
                name: Some("lab".parse().unwrap()),
                kind: MemberKind::Computer,
                expires: at(),
            },
            AccountChange::SetPassword { target: account(), password: "fixture".into() },
            AccountChange::OpenAccount { name: "bo".parse().unwrap(), password: "fixture".into() },
            AccountChange::SetAccount { name: "bo".parse().unwrap(), locked: true },
            AccountChange::Transfer { to: "bo".parse().unwrap() },
            AccountChange::DeleteAccount { name: "bo".parse().unwrap() },
            AccountChange::Settings { self_registration: false },
            AccountChange::ShareProject { town: "mark50".parse().unwrap(), with: "bo".parse().unwrap(), shared: true },
            AccountChange::MoveProject { town: "mark50".parse().unwrap(), to: "bo".parse().unwrap() },
            AccountChange::Seen { name: "pixel".parse().unwrap() },
        ],
        connections: vec![
            ConnectionState { mode: ConnectionMode::Local, device: None },
            ConnectionState {
                mode: ConnectionMode::Device,
                device: Some(DeviceState {
                    name: Some("pixel".parse().unwrap()),
                    hubs: vec![DeviceHub { hub: "home".parse().unwrap(), link: online(), logins: vec![account()] }],
                }),
            },
            ConnectionState { mode: ConnectionMode::Device, device: Some(DeviceState { name: None, hubs: vec![] }) },
        ],
        choices: vec![
            Choices::default(),
            Choices {
                account: Some(HubAccount { hub: "w-0123456789abcdef".parse().unwrap(), account: account() }),
                focus: fastctx_types::focus::Focus {
                    machines: vec![
                        fastctx_types::focus::FocusedMachine {
                            machine: "lab.home".parse().unwrap(),
                            folders: vec!["C:/work/中文 项目".into(), "D:/data".into()],
                        },
                        fastctx_types::focus::FocusedMachine { machine: "laptop".parse().unwrap(), folders: vec![] },
                    ],
                },
                home: Some("C:/Users/ada/.claude-work".into()),
                files_on: None,
            },
            Choices { files_on: Some("laptop".parse::<Written>().unwrap()), ..Choices::default() },
        ],
    }
}
