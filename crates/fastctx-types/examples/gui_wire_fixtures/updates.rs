//! Frozen update states and account-view wire shapes.
use fastctx_types::update::*;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    states: Vec<State>,
    checks: Vec<CheckFailure>,
    reasons: Vec<FailureReason>,
    calls: Vec<ViewCall>,
    device_calls: Vec<UpdateCall>,
    replies: Vec<ViewReply>,
}

pub fn fixtures() -> Fixtures {
    let version = "1.0.2".to_string();
    let at: jiff::Timestamp = "2026-10-20T12:00:00Z".parse().unwrap();
    let reasons = vec![
        FailureReason::NoPackage,
        FailureReason::Signature,
        FailureReason::Disk,
        FailureReason::NotNewer,
        FailureReason::Other,
        FailureReason::Installer { code: Some(23) },
        FailureReason::Sources { sources: vec![SourceFailure { via: Via::Github, why: "connection refused".into() }] },
    ];
    let problem = Problem::new(FailureReason::Signature, "the package does not match its signature");
    let stages = vec![
        Stage::Idle,
        Stage::Unknown,
        Stage::Fetching { version: version.clone(), via: None, received: 0, total: None },
        Stage::Fetching { version: version.clone(), via: Some(Via::Github), received: 1, total: Some(1024) },
        Stage::Ready { version: version.clone() },
        Stage::WaitingIdle { version: version.clone() },
        Stage::WaitingApp { version: version.clone() },
        Stage::Installing { version: version.clone() },
        Stage::Administrator { version: version.clone() },
        Stage::Renewing { version: version.clone(), phase: fastctx_types::renewal::Phase::ReturningToService },
        Stage::Failed { version: version.clone(), problem: problem.clone() },
    ];
    let manifest = Manifest {
        version: version.clone(),
        notes: Some("An update.".into()),
        pub_date: Some(at.to_string()),
        platforms: Default::default(),
        fastctx: Default::default(),
    };
    let due = Due {
        version: version.clone(),
        by: "owner".parse().unwrap(),
        at,
        manifest: Some(manifest.clone()),
        problem: Some(problem),
    };
    let checks = vec![
        CheckFailure::Unreachable { reason: ReachFailure::Dns },
        CheckFailure::Unreachable { reason: ReachFailure::Connect },
        CheckFailure::Unreachable { reason: ReachFailure::Tls },
        CheckFailure::Unreachable { reason: ReachFailure::Timeout },
        CheckFailure::Unreachable { reason: ReachFailure::Interrupted },
        CheckFailure::Unreachable { reason: ReachFailure::Proxy },
        CheckFailure::Unreachable { reason: ReachFailure::Other },
        CheckFailure::RateLimited { until: Some(at) },
        CheckFailure::Http { status: 503 },
        CheckFailure::Unreadable,
        CheckFailure::Other,
    ];
    let states = stages
        .into_iter()
        .map(|stage| State {
            stage,
            available: Some(manifest.clone()),
            automatic: Some(version.clone()),
            above_hub: true,
            progress: true,
            preferences: true,
            looked: Looked {
                at: Some(at),
                checking: true,
                failure: Some(checks[0].clone()),
                github_at: Some(at),
                next: Some(at),
                failures: 2,
                ..Default::default()
            },
            network: Some(Network {
                updates_view: true,
                due: [("server".parse().unwrap(), due.clone())].into(),
                ..Default::default()
            }),
            failed: Some(Failed {
                version: version.clone(),
                target: "linux-x86_64-appimage".into(),
                why: "disk full".into(),
                reason: Some(FailureReason::Disk),
            }),
            ..Default::default()
        })
        .collect();
    let machine = MachineUpdate {
        repair_supported: true,
        repair_due: None,
        repair: None,
        machine: "server".parse().unwrap(),
        owner: "owner".parse().unwrap(),
        group: None,
        kind: Kind::Headless,
        online: false,
        reported: Some(at),
        kernel: Some("1.0.1".into()),
        app: None,
        target: Some("fastctx-x86_64-unknown-linux-gnu".into()),
        channel: Channel::Stable,
        auto: Some(true),
        auto_chosen: false,
        relays: Some(true),
        github: Some(false),
        github_at: Some(at),
        check_failure: Some(checks[0].clone()),
        checking: false,
        stage: Stage::Idle,
        due: Some(due),
        offer: Some(manifest.clone()),
        actions: Actions {
            repair: None,
            update: Permission::yes(),
            choose: Permission::yes(),
            settings: Permission::no(ActionReason::Offline),
            schedule: Permission::yes(),
            cancel: Permission::yes(),
        },
    };
    Fixtures {
        device_calls: vec![
            UpdateCall::Preferences { auto: Some(true), relays: None },
            UpdateCall::Progress { stage: Stage::WaitingIdle { version: "1.0.2".into() } },
            UpdateCall::Progress { stage: Stage::Idle },
        ],
        states,
        checks,
        reasons,
        calls: vec![
            ViewCall::View,
            ViewCall::Release { version: "1.0.2".into() },
            ViewCall::Schedule { machines: vec!["server".parse().unwrap()], version: Some(version) },
            ViewCall::Schedule { machines: vec!["server".parse().unwrap()], version: None },
            ViewCall::Policy { minimum: None, managed_auto: true },
        ],
        replies: vec![
            ViewReply::View {
                view: Box::new(View {
                    repair_supported: true,
                    hub: "1.0.1".into(),
                    stable: Some(manifest.clone()),
                    beta: None,
                    versions: vec![
                        VersionChoice {
                            version: manifest.version.clone(),
                            published: manifest.pub_date.clone(),
                            summary: manifest.notes.clone(),
                            manifest: Some(manifest.clone()),
                            problem: None,
                        },
                        VersionChoice {
                            version: "1.0.3".into(),
                            published: None,
                            summary: None,
                            manifest: None,
                            problem: Some(Problem::new(FailureReason::Other, "manifest unavailable")),
                        },
                    ],
                    releases: vec![manifest],
                    policy: Some(Policy::default()),
                    machines: vec![machine],
                    available: 1,
                }),
            },
            ViewReply::Scheduled {
                results: vec![Scheduled {
                    machine: "server".parse().unwrap(),
                    done: false,
                    reason: Some(ActionReason::TooOld),
                    why: Some("update this machine first".into()),
                }],
            },
            ViewReply::Release {
                choice: Box::new(VersionChoice {
                    version: "1.0.3".into(),
                    published: None,
                    summary: None,
                    manifest: None,
                    problem: Some(Problem::new(FailureReason::Other, "manifest unavailable")),
                }),
            },
            ViewReply::Done,
        ],
    }
}
