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
        }],
        calls,
        diff,
    }
}
