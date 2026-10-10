use fastctx_types::ProcessIdentity;
use fastctx_types::ipc::Welcome;
use fastctx_types::renewal::{
    Action, Build, MAINTENANCE, Mismatch, PathCommand, PathLookup, Phase, Request, Response, Running, Status,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub requests: Vec<Request>,
    pub responses: Vec<Response>,
    pub statuses: Vec<Status>,
    pub refused: Welcome,
}

pub fn fixtures() -> Fixtures {
    let build = Build { version: "1.0.0".into(), protocol: 6, digest: "ab".repeat(32), built: Some(1_791_600_000) };
    let mismatch =
        Mismatch { node_version: "1.0.0".into(), node_protocol: 5, client_version: "1.0.0".into(), client_protocol: 6 };
    let node = Running {
        build: build.clone(),
        executable: "C:/Users/example/.fastctx/bin/fastctx.exe".into(),
        process: ProcessIdentity { pid: 123, token: "process-instance".into() },
        yielding: false,
        bridge: true,
        supervised: false,
    };
    let phases = [
        Phase::Staged,
        Phase::Preparing,
        Phase::WaitingForIdle,
        Phase::AwaitingLegacyExit,
        Phase::AwaitingAdministrator,
        Phase::UpdatingGuardian,
        Phase::ReturningToService,
        Phase::Complete,
        Phase::Failed,
    ];
    Fixtures {
        requests: [Action::Status, Action::Yield, Action::Cancel]
            .into_iter()
            .map(|action| Request { maintenance: MAINTENANCE, action })
            .collect(),
        responses: vec![
            Response::Status { node: node.clone() },
            Response::Done,
            Response::Refused { reason: "handover already committed".into() },
        ],
        statuses: phases
            .into_iter()
            .map(|phase| Status {
                client: Some(build.clone()),
                installed: Some(build.clone()),
                commands: vec![PathCommand {
                    lookup: PathLookup::Windows,
                    path: "C:/npm/fastctx.cmd".into(),
                    executable: Some("C:/npm/node_modules/@fastctx/win32-x64/bin/fastctx.exe".into()),
                    build: Some(Build { version: "0.2.6".into(), protocol: 0, digest: "ef".repeat(32), built: None }),
                    is_world: Some(false),
                    legacy_unapply: true,
                    error: None,
                }],
                node: (phase != Phase::AwaitingLegacyExit).then(|| node.clone()),
                mismatch: (phase == Phase::AwaitingLegacyExit).then(|| mismatch.clone()),
                pending: (phase != Phase::Complete).then(|| build.clone()),
                error: (phase == Phase::Failed).then(|| "a staged file is locked".into()),
                phase: Some(phase),
            })
            .chain([Status {
                client: Some(build.clone()),
                installed: Some(build.clone()),
                commands: Vec::new(),
                node: None,
                mismatch: Some(mismatch.clone()),
                pending: Some(build.clone()),
                phase: Some(Phase::Staged),
                error: None,
            }])
            .collect(),
        refused: Welcome::Refused { reason: "incompatible business protocol".into(), mismatch: Some(mismatch) },
    }
}
