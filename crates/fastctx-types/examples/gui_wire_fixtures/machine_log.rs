use fastctx_types::accounts::{MemberKind, PlanLabel, Relay, TokenWay};
use fastctx_types::machine_log::{Action, Actor, LoginMethod, Outcome, Page, Record};

pub fn fixtures() -> Vec<Page> {
    let account = || "ada".parse().unwrap();
    let to = || "bo".parse().unwrap();
    let machine = || "laptop".parse().unwrap();
    let town = || fastctx_types::access::TownId::try_from("t-0123456789abcdef".to_string()).unwrap();
    let at = "2026-10-10T12:00:00Z".parse().unwrap();
    let actions = vec![
        Action::Collab { version: 2 },
        Action::Share { machine: machine(), to: to(), shared: true, jobs: Some(8) },
        Action::Assist {
            machine: machine(),
            to: to(),
            agent: "codex".parse().unwrap(),
            home: Some("/home/me/agent".into()),
            assisting: false,
            until: None,
            weekly: None,
            daily: None,
            five_hour: None,
            turns: None,
        },
        Action::Assist {
            machine: machine(),
            to: to(),
            agent: "claude".parse().unwrap(),
            home: None,
            assisting: true,
            until: Some(at),
            weekly: Some(fastctx_types::usage::Usd(25_000_000)),
            daily: Some(fastctx_types::usage::Usd(5_000_000)),
            five_hour: Some(75),
            turns: Some(6),
        },
        Action::AssistEnded { machine: machine(), to: to(), agent: "claude".parse().unwrap(), home: None },
        Action::Plan { machine: machine(), agent: "claude".parse().unwrap(), home: None, plan: Some(PlanLabel::Free) },
        Action::StopActing { machine: machine() },
        Action::Relays {
            machine: machine(),
            relays: vec![Relay { label: "relay".into(), addresses: vec!["203.0.113.20:7444".parse().unwrap()] }],
        },
        Action::RelayAt { machine: machine(), addresses: vec![] },
        Action::Direct { machine: machine(), at: vec!["203.0.113.20:7444".parse().unwrap()] },
        Action::Direct { machine: machine(), at: vec![] },
        Action::Remove { name: machine(), reason: "retired".into(), then: None },
        Action::Remove {
            name: machine(),
            reason: "retired".into(),
            then: Some(fastctx_types::uninstall::Then::Everything),
        },
        Action::UninstallLeft {
            name: machine(),
            left: "the service needs an administrator to remove".into(),
            by: Some("ada".parse().unwrap()),
            home: Some("ada".parse().unwrap()),
        },
        Action::UpdateDue { machine: machine(), version: Some("1.0.1".into()) },
        Action::UpdateDue { machine: machine(), version: None },
        Action::Updated {
            machine: machine(),
            from: "1.0.1-beta.3".into(),
            to: "1.0.1".into(),
            by: "ada".parse().unwrap(),
        },
        Action::Rename { machine: machine(), to: "gpu-2".parse().unwrap() },
        Action::Group { machine: machine(), group: Some("lab A".into()) },
        Action::RenameGroup { owner: "ada".parse().unwrap(), from: "lab A".into(), to: None },
        Action::Logout,
        Action::Token {
            register: false,
            name: None,
            kind: MemberKind::Phone,
            expires: at,
            managed: false,
            label: None,
            uses: None,
            way: None,
        },
        Action::Token {
            register: true,
            name: None,
            kind: MemberKind::Computer,
            expires: at,
            managed: true,
            label: Some("机房 A".into()),
            uses: Some(40),
            way: Some(TokenWay::Paste),
        },
        Action::RevokeToken { label: Some("机房 A".into()), way: Some(TokenWay::Paste), managed: true },
        Action::RevokeToken { label: None, way: None, managed: false },
        Action::SetRole { name: to(), visitor: true },
        Action::AcceptRole { accept: true },
        Action::PasswordStart { target: account() },
        Action::PasswordFinish { target: account() },
        Action::OpenAccount { name: to(), visitor: false },
        Action::SetAccount { name: to(), locked: true },
        Action::Transfer { to: to() },
        Action::DeleteAccount { name: to() },
        Action::Settings { self_registration: true },
        Action::OpenProject { town: town(), name: "research".parse().unwrap() },
        Action::ShareProject { town: town(), name: Some("research".parse().unwrap()), with: to(), shared: true },
        Action::MoveProject { town: town(), name: None, to: to() },
        Action::Joined {
            name: machine(),
            kind: MemberKind::Computer,
            registered: true,
            via: LoginMethod::Password,
            managed: false,
        },
        Action::Joined {
            name: machine(),
            kind: MemberKind::Web,
            registered: false,
            via: LoginMethod::Token,
            managed: false,
        },
        Action::Joined {
            name: machine(),
            kind: MemberKind::Computer,
            registered: true,
            via: LoginMethod::Token,
            managed: true,
        },
        Action::AllowActing { machine: machine(), agent: "claude".parse().unwrap(), home: None },
        Action::ActingStopped { machine: machine(), account: account() },
        Action::Left {
            name: machine(),
            account: account(),
            kind: MemberKind::Computer,
            unregistered: true,
            reason: "logged out".into(),
        },
    ];
    let length = actions.len();
    let records = actions
        .into_iter()
        .enumerate()
        .map(|(index, action)| Record {
            id: (length - index) as u64,
            at,
            actor: if index == 11 {
                Actor {
                    account: None,
                    authenticated: false,
                    member: None,
                    peer: Some("203.0.113.10:4567".parse().unwrap()),
                }
            } else {
                Actor { account: Some(account()), authenticated: true, member: Some(machine()), peer: None }
            },
            action,
            outcome: if index == 2 {
                Outcome::Refused { reason: "the machine is another account's".into() }
            } else {
                Outcome::Done
            },
        })
        .collect();
    vec![Page { records, next_before: Some(1) }, Page { records: vec![], next_before: None }]
}
