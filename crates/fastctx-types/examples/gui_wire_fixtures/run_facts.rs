use fastctx_types::run::{ExitStatus, RunState};
use fastctx_types::run_facts::*;
use fastctx_types::write::ChangeKind;
use serde::Serialize;

#[derive(Serialize)]
pub struct Fixtures {
    pub calls: Vec<RunFacts>,
    pub diff: Vec<DiffLine>,
    pub updates: Vec<JobFacts>,
    pub projected_reads: Vec<fastctx_types::lines::ProjectedLines>,
    pub parts: Vec<fastctx_types::lines::Part>,
    pub open_errors: Vec<fastctx_types::ipc::Failure>,
}

pub fn example() -> RunFacts {
    RunFacts {
        turn: None, record: None, notes: vec![OutputNoteMatch {
            note: OutputNote { command: NoteCommand::Write, write: Some("00112233445566778899aabbccddeeff".into()) },
            from: 0,
            to: 42,
        }],
        machine: "laptop".parse().unwrap(),
        state: RunState::Ended { status: ExitStatus::Code(0) },
        job: Some("j-abc123".parse().unwrap()),
        output: Some(OutputRef {
            policy: None,
            content: None,
            machine: "laptop".parse().unwrap(),
            path: "/tmp/output.log".into(),
            bytes: 90000,
            lines: 4000,
            cut: true,
        }),
        visited: vec![("gpu".parse().unwrap(), "j-def456".parse().unwrap())],
        steps: steps(),
        writes: [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| WriteFact {
                turn: None, note: Some(if i == 0 {
                    WriteNote::Text {
                        command: NoteCommand::Write,
                        text: "=== write /work/file on gpu (created) ===\n".into(),
                    }
                } else {
                    WriteNote::Same { fact: "00112233445566778899aabbccddeeff-0".into() }
                }),
                id: format!("00112233445566778899aabbccddeeff-{i}"),
                machine: "gpu".parse().unwrap(),
                path: format!("/work/file-{i}.txt"),
                kind,
                added: Some(1),
                removed: Some(1),
                unavailable: None,
                diff: Some(DiffRef {
                    content: None,
                    machine: "gpu".parse().unwrap(),
                    path: format!("/tmp/diff-{i}.jsonl"),
                    bytes: Some(if i == 0 { 180 } else { 70_000 }),
                    lines: (i == 0).then(|| {
                        vec![
                            DiffLine {
                                kind: DiffLineKind::Removed,
                                before: Some(1),
                                after: None,
                                text: "port = 8080".into(),
                                ending: "\n".into(),
                            },
                            DiffLine {
                                kind: DiffLineKind::Added,
                                before: None,
                                after: Some(1),
                                text: "port = 8081".into(),
                                ending: "\n".into(),
                            },
                        ]
                    }),
                }),
            })
            .collect(),
        spans: vec![
            OutputSpan {
                note: Some(OutputNote {
                    command: NoteCommand::Write,
                    write: Some("00112233445566778899aabbccddeeff".into()),
                }),
                piece: DisplayPiece::Text { first_line: 1, text: "a World note\n".into() },
            },
            OutputSpan { note: None, piece: DisplayPiece::Text { first_line: 2, text: "program output\n".into() } },
            OutputSpan {
                note: None,
                piece: DisplayPiece::PartOfLine { line: 3, from: 2, to: 4, of: 20, text: "😀中x".into() },
            },
            OutputSpan { note: None, piece: DisplayPiece::Omitted { first_line: 4, last_line: 3998 } },
            OutputSpan {
                note: None,
                piece: DisplayPiece::Image { image: 0, mime: "image/png".into(), width: 8, height: 9 },
            },
            OutputSpan { note: None, piece: DisplayPiece::Unreadable { line: 4000, reason: "broken graphic".into() } },
        ],
    }
}

pub fn fixtures() -> Fixtures {
    let mut calls = vec![example()];
    for state in [
        RunState::Running,
        RunState::Exited { status: ExitStatus::Code(7), remaining: 1 },
        RunState::Stopped,
        RunState::Interrupted { reason: "supervisor ended".into() },
        RunState::Failed { error: "could not launch".into() },
    ] {
        let mut facts = example();
        facts.state = state;
        facts.job = None;
        facts.output = None;
        facts.writes[0].diff = None;
        facts.writes[0].added = None;
        facts.writes[0].removed = None;
        facts.writes[0].unavailable = Some("undecodable bytes".into());
        calls.push(facts);
    }
    let diff = vec![
        DiffLine {
            kind: DiffLineKind::Hunk,
            before: Some(1),
            after: Some(1),
            text: String::new(),
            ending: String::new(),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            before: Some(1),
            after: Some(1),
            text: "same".into(),
            ending: "\n".into(),
        },
        DiffLine {
            kind: DiffLineKind::Removed,
            before: Some(2),
            after: None,
            text: "before".into(),
            ending: "\r\n".into(),
        },
        DiffLine {
            kind: DiffLineKind::Added,
            before: None,
            after: Some(2),
            text: "after".into(),
            ending: String::new(),
        },
    ];
    Fixtures {
        open_errors: vec![
            fastctx_types::ipc::Failure::output_not_retained("the output job is no longer retained"),
            fastctx_types::ipc::Failure::source_missing("the source does not exist"),
            fastctx_types::ipc::Failure::new("the connection broke"),
        ],
        projected_reads: vec![fastctx_types::lines::ProjectedLines {
            start: 3,
            lines: vec!["😀 note".into()],
            notes: vec![fastctx_types::lines::LineNote {
                line: 3,
                from: 3,
                to: 7,
                note: OutputNote {
                    command: NoteCommand::Write,
                    write: Some("00112233445566778899aabbccddeeff".into()),
                },
            }],
        }],
        parts: vec![
            fastctx_types::lines::Part { utf16_length: None, line: 3, text: "😀 ".into(), ended: false, note: None },
            fastctx_types::lines::Part {
                utf16_length: None,
                line: 3,
                text: "note".into(),
                ended: true,
                note: Some(OutputNote {
                    command: NoteCommand::Write,
                    write: Some("00112233445566778899aabbccddeeff".into()),
                }),
            },
        ],
        updates: vec![JobFacts {
            record: None, script: None, started_at: None, finished_at: None,
            turn: None, notes: None,
            call_id: "host-call-1".into(),
            primary: true,
            output: example().output,
            visited: example().visited,
            machine: "laptop".parse().unwrap(),
            job: "j-abc123".parse().unwrap(),
            sequence: 2,
            state: Some(RunState::Running),
            writes: example().writes,
            steps: steps().into_iter().take(2).collect(),
        }],
        calls,
        diff,
    }
}

/// One step of every kind, as a run of `j-abc123` on laptop told them.
fn steps() -> Vec<RunStep> {
    let at = |minute: i64| jiff::Timestamp::from_second(1_792_000_000 + minute * 60).unwrap();
    let step = |n: i64, machine: &str, job: &str, did: StepDid| RunStep {
        id: format!("step-{n}"),
        job: job.parse().unwrap(),
        machine: machine.parse().unwrap(),
        began: at(n),
        ended: (n != 1).then(|| at(n + 1)),
        did,
    };
    let place = |machine: &str, path: &str| StepPlace { machine: machine.parse().unwrap(), path: path.into() };
    vec![
        step(0, "laptop", "j-abc123", StepDid::On {
            to: "gpu".parse().unwrap(),
            command: "cat /data/fit.csv".into(),
            sent: Some("j-def456".parse().unwrap()),
            end: Some(StepEnd::Exited { code: 0 }),
            pipes_to: Some("lab".parse().unwrap()),
            fed_by: None,
            tail: None,
        }),
        step(1, "laptop", "j-abc123", StepDid::On {
            to: "lab".parse().unwrap(),
            command: "wc -l".into(),
            sent: Some("j-ghi789".parse().unwrap()),
            end: None,
            pipes_to: None,
            fed_by: Some("j-def456".parse().unwrap()),
            tail: Some("12040".into()),
        }),
        step(2, "gpu", "j-def456", StepDid::Copy {
            from: vec![place("gpu", "/data/fit.csv")],
            to: place("lab", "/srv/in/"),
            files: 1,
            bytes: 4_200_000,
            unchanged: 0,
            problems: vec!["/data/locked.csv: permission denied".into()],
        }),
        step(3, "laptop", "j-abc123", StepDid::View {
            paths: vec!["/work/plot.png".into(), "/work/paper.pdf".into()],
            pictures: vec![
                StepPicture { path: "/work/plot.png".into(), page: None, width: 1200, height: 800, mime: "image/png".into() },
                StepPicture { path: "/work/paper.pdf".into(), page: Some(3), width: 1545, height: 2000, mime: "image/png".into() },
            ],
        }),
        step(4, "laptop", "j-abc123", StepDid::Wait {
            jobs: vec![
                WaitedJob {
                    job: "j-k3x9a2".parse().unwrap(),
                    machine: "laptop".parse().unwrap(),
                    at: at(3),
                    end: StepEnd::Exited { code: 1 },
                    tail: Some("error: 2 tests failed".into()),
                    command: Some("cargo test --workspace".into()),
                },
                WaitedJob {
                    job: "j-q8w2e1".parse().unwrap(),
                    machine: "laptop".parse().unwrap(),
                    at: at(4),
                    end: StepEnd::Lost { why: "the machine restarted".into() },
                    tail: None,
                    command: None,
                },
            ],
        }),
        step(5, "laptop", "j-abc123", StepDid::Stop { what: "j-ghi789".into(), on: Some("lab".parse().unwrap()), how: StopHow::Stopped }),
        step(6, "laptop", "j-abc123", StepDid::Stop { what: "s-9f2c".into(), on: None, how: StopHow::AlreadyStopped }),
        step(7, "laptop", "j-abc123", StepDid::Page { name: "fit".into(), town: "ingest".parse().unwrap(), title: "Fit".into(), version: 4 }),
        step(8, "laptop", "j-abc123", StepDid::Project { town: "ingest".parse().unwrap() }),
        step(9, "laptop", "j-abc123", StepDid::Plan { version: 7 }),
        step(10, "laptop", "j-abc123", StepDid::Goal { mark: GoalMark::Complete }),
        step(11, "laptop", "j-abc123", StepDid::Sense { sentinel: "s-9f2c".into(), condition: "the training loss stops falling".into() }),
        step(12, "laptop", "j-abc123", StepDid::On {
            to: "server".parse().unwrap(),
            command: "git --version".into(),
            sent: None,
            end: Some(StepEnd::NotRun { why: "server is offline".into() }),
            pipes_to: None,
            fed_by: None,
            tail: None,
        }),
        step(13, "laptop", "j-abc123", StepDid::On {
            to: "lab".parse().unwrap(),
            command: "sleep 600".into(),
            sent: Some("j-z0z0z0".parse().unwrap()),
            end: Some(StepEnd::Stopped),
            pipes_to: None,
            fed_by: None,
            tail: None,
        }),
    ]
}
