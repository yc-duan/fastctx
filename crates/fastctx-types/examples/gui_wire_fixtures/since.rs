use fastctx_types::project::{Location, Writer};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Host, Origin, Outcome};
use fastctx_types::run::{EndedHow, ExitStatus, Exited};
use fastctx_types::session::{TurnEnd, TurnHow};
use fastctx_types::since::*;
use fastctx_types::{Address, Call, Reply};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    calls: Vec<Call>,
    pub(super) replies: Vec<Reply>,
    remote_calls: Vec<RemoteCall>,
    remote_replies: Vec<RemoteReply>,
}

pub fn fixtures() -> Fixtures {
    let at: Timestamp = "2026-10-04T12:06:07.123456Z".parse().unwrap();
    let resident: Address = "codex-ab12@desktop".parse().unwrap();
    let asker: Address = "claude-cd34@laptop".parse().unwrap();
    let town = Some("project-0".parse().unwrap());
    let source = Location {
        machine: resident.machine.clone(),
        path: "C:/world/outside/residents/codex-ab12/turns/example.json".into(),
    };
    let host = Host { agent: "codex".parse().unwrap(), client: "codex app-server".into(), version: "fixture".into() };
    let mut kinds = vec![
        SinceKind::FileAction {
            resident: resident.clone(),
            hand: "edit".into(),
            paths: "src/pid.rs".into(),
            result: "1 change".into(),
        },
        SinceKind::ResidentBorn { resident: resident.clone(), host: host.clone(), origin: Origin::User },
        SinceKind::ResidentBorn {
            resident: resident.clone(),
            host,
            origin: Origin::Colleague { asked_by: asker.clone(), label: "Check 世界 records".into() },
        },
        SinceKind::ColleagueAsked {
            asker: asker.clone(),
            colleague: resident.clone(),
            label: "Check 世界 records".into(),
        },
        SinceKind::ResidentMoved { resident: resident.clone(), from: None, to: town.clone() },
        SinceKind::ClaudeRegistered {
            resident: asker.clone(),
            session: "session-1".into(),
            instance: Some("activation-1".into()),
        },
        SinceKind::MachineConnected,
        SinceKind::MachineOffline,
        SinceKind::SubmissionUncertain {
            resident: resident.clone(),
            delivery: "n-ab12".parse().unwrap(),
            why: "host stopped before receipt".into(),
            end: None,
            readers: Some(vec![asker.clone()]),
        },
    ];
    for how in [TurnHow::Completed, TurnHow::Failed, TurnHow::Interrupted, TurnHow::Other("future-outcome".into())] {
        kinds.push(SinceKind::TurnEnded {
            resident: resident.clone(),
            turn: "turn-1".into(),
            end: TurnEnd {
                facts: None,
                how,
                reason: Some("Host's exact reason 世界".into()),
                error: None,
                duration_ms: Some(1234),
                last_message: Some("Answer, verbatim.".into()),
            },
            readers: Some(vec![asker.clone()]),
        });
    }
    kinds.push(SinceKind::TurnEnded {
        resident: resident.clone(),
        turn: String::new(),
        end: TurnEnd {
            facts: None,
            how: TurnHow::Failed,
            reason: None,
            error: None,
            duration_ms: None,
            last_message: None,
        },
        readers: None,
    });
    for how in [
        EndedHow::Finished,
        EndedHow::Stopped,
        EndedHow::Interrupted { reason: "supervisor disappeared".into() },
        EndedHow::Failed { error: "spawn failed".into() },
    ] {
        kinds.push(SinceKind::JobEnded {
            job: "j-ab12cd".parse().unwrap(),
            owner: asker.clone().into(),
            started_at: at,
            elapsed_ms: 0,
            how,
            exit: Some(Exited { status: ExitStatus::Code(7), at }),
            script: "exit 7".into(),
            output: "C:/world/output.log".into(),
            account: None,
        });
    }
    for outcome in [
        Outcome::Delivered,
        Outcome::Waiting { why: None },
        Outcome::Sent,
        Outcome::Failed { reason: "offline".into() },
    ] {
        kinds.push(SinceKind::AnswerDelivery {
            colleague: resident.clone(),
            to: asker.clone(),
            turn: Some("turn-1".into()),
            outcome,
        });
    }
    for by in [
        Writer::User,
        Writer::Resident { address: resident.clone() },
        Writer::Session { agent: "claude".parse().unwrap() },
    ] {
        kinds.push(SinceKind::JournalEntry {
            journal: "measurements".parse().unwrap(),
            entry: "20261004T120607Z-desktop".parse().unwrap(),
            by: by.clone(),
        });
        kinds.push(SinceKind::JournalDescription {
            journal: "measurements".parse().unwrap(),
            version: 2,
            by: by.clone(),
        });
        kinds.push(SinceKind::ProjectVersion { version: 2, name: "Example 世界".into(), by });
    }
    let events: Vec<_> = kinds
        .into_iter()
        .enumerate()
        .map(|(index, what)| SinceEvent {
            id: format!("event-{index}"),
            at,
            machine: resident.machine.clone(),
            town: match index % 3 {
                0 => SinceTown::Known { town: town.clone() },
                1 => SinceTown::Known { town: None },
                _ => SinceTown::Unrecorded { recorded_in: vec![None, town.clone()] },
            },
            sources: vec![source.clone()],
            what,
        })
        .collect();
    let coverage = SinceCoverage {
        machine: resident.machine.clone(),
        read_started_at: at,
        read_finished_at: at,
        issues: vec![SinceIssue { source: "C:/world/record.json".into(), reason: "unreadable record".into() }],
    };
    let undated: Vec<_> = [
        UndatedKind::Asleep,
        UndatedKind::ClaudeClosed { session: "session-1".into(), instance: "activation-1".into() },
        UndatedKind::ClaudeRegistered { session: "legacy-session".into(), instance: None },
        UndatedKind::SubmissionUncertain {
            delivery: "n-ab12".parse().unwrap(),
            why: "host stopped before receipt".into(),
            end: None,
            readers: None,
        },
    ]
    .into_iter()
    .map(|what| UndatedFact {
        resident: resident.clone(),
        town: SinceTown::Unrecorded { recorded_in: vec![town.clone()] },
        source: source.clone(),
        what,
    })
    .collect();
    let reply = SinceReply { coverage: coverage.clone(), events: events.clone(), undated: undated.clone() };
    let world = WorldSince {
        machine: resident.machine.clone(),
        since: at,
        town: town.clone(),
        machines: vec![
            MachineSince::Answered { reply: coverage },
            MachineSince::Offline { machine: "laptop".parse().unwrap(), since: Some(at) },
            MachineSince::Offline { machine: "server".parse().unwrap(), since: None },
            MachineSince::NoAnswer { machine: "workstation".parse().unwrap(), reason: "the connection broke".into() },
        ],
        events,
        undated,
        issues: Vec::new(),
        limitations: vec!["No earlier presence intervals are retained.".into()],
    };
    Fixtures {
        calls: vec![Call::Since { since: at, town: town.clone() }, Call::Since { since: at, town: None }],
        replies: vec![Reply::Since(world)],
        remote_calls: vec![RemoteCall::Since { since: at, town }],
        remote_replies: vec![RemoteReply::Since(reply)],
    }
}
