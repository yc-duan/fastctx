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
    LinkState::Online { since: at(), hub: "203.0.113.10:7443".into(), relay: Some("la".into()), route: Route::Local }
}

fn hub() -> HubAccounts {
    HubAccounts {
        hub: "w-0123456789abcdef".parse().unwrap(),
        name: "home".parse().unwrap(),
        addresses: vec!["203.0.113.10:7443".parse().unwrap()],
        link: online(),
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
                kind: MemberKind::Computer,
                logins: vec!["ada".parse().unwrap()],
                home: Some("ada".parse().unwrap()),
                relays: vec![Relay { label: "la".into(), addresses: vec!["203.0.113.20:7444".parse().unwrap()] }],
                added: at(),
                online: true,
                since: Some(at()),
                through: Some("la".into()),
                key: "5be1fcee".into(),
            },
            MemberSeen {
                name: "pixel".parse().unwrap(),
                kind: MemberKind::Phone,
                logins: vec!["ada".parse().unwrap()],
                home: None,
                relays: vec![],
                added: at(),
                online: false,
                since: None,
                through: None,
                key: "4f2a91aa".into(),
            },
        ],
        shares: vec![ShareSeen { machine: "desktop".parse().unwrap(), account: "bo".parse().unwrap() }],
        lends: vec![
            LendSeen {
                machine: "desktop".parse().unwrap(),
                account: "cy".parse().unwrap(),
                agent: "codex".parse().unwrap(),
                home: None,
            },
            LendSeen {
                machine: "gpu".parse().unwrap(),
                account: "ada".parse().unwrap(),
                agent: "claude".parse().unwrap(),
                home: Some("/home/bo/.claude-team".into()),
            },
        ],
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
    }
}

pub fn fixtures() -> Fixtures {
    let machine = || "desktop".parse().unwrap();
    let account = || "ada".parse().unwrap();
    Fixtures {
        replies: vec![
            AccountsReply { machine: machine(), hubs: vec![hub()] },
            AccountsReply { machine: machine(), hubs: vec![] },
        ],
        changes: vec![
            AccountChange::Share { machine: machine(), to: "bo".parse().unwrap(), shared: true },
            AccountChange::Lend {
                machine: machine(),
                to: "cy".parse().unwrap(),
                agent: "codex".parse().unwrap(),
                home: Some("C:/Users/ada/.codex-work".into()),
                lent: true,
            },
            AccountChange::StopActing { machine: "gpu".parse().unwrap() },
            AccountChange::Relays {
                machine: machine(),
                relays: vec![Relay { label: "la".into(), addresses: vec!["203.0.113.20:7444".parse().unwrap()] }],
            },
            AccountChange::Remove { name: "pixel".parse().unwrap(), reason: "lost".into() },
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
                works_on: Some("lab.home".parse::<Written>().unwrap()),
                colleagues: Some("gpu".parse::<Written>().unwrap()),
                home: Some("C:/Users/ada/.claude-work".into()),
                colleagues_home: Some("/home/ada/.codex-gpu".into()),
                files_on: None,
            },
            Choices { files_on: Some("laptop".parse::<Written>().unwrap()), ..Choices::default() },
        ],
    }
}
