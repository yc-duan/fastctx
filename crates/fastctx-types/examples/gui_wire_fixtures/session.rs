use fastctx_types::Address;
use fastctx_types::session::*;
use jiff::Timestamp;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
pub struct Fixtures {
    pub events: Vec<SessionEvent>,
    pub watched: Vec<Watched>,
    pub health_cases: Vec<HealthCase>,
}

#[derive(Serialize)]
pub struct HealthCase {
    pub change: SessionChange,
    pub health: Option<Health>,
}

fn at() -> Timestamp {
    "2026-10-04T05:06:07.123456789-07:00".parse().unwrap()
}

fn address() -> Address {
    "codex-ab12@desktop".parse().unwrap()
}

fn event(happened: Happened) -> SessionEvent {
    SessionEvent { at: at(), happened }
}

fn item(id: &str, what: What) -> Item {
    Item { draft: None, id: id.into(), started_at: None, detail: None, what }
}

fn changes() -> Vec<SessionChange> {
    vec![
        SessionChange::RunFactsUpdated {
            call_id: "host-call-1".into(),
            update: Box::new(super::run_facts::fixtures().updates[0].clone()),
        },
        SessionChange::HistoryChanged,
        SessionChange::Forked { source: address(), thread: "source-thread".into(), through: None },
        SessionChange::Forked {
            source: address(),
            thread: "source-thread".into(),
            through: Some("source-turn".into()),
        },
        SessionChange::Diff { diff: "-before\n+after\n".into() },
        SessionChange::Terminal { item: "tool-1".into(), process: "process-1".into(), input: "yes\n".into() },
        SessionChange::Usage { usage: json!({"inputTokens": 42}) },
        SessionChange::Status { status: json!("idle"), waiting_for_user: None },
        SessionChange::Status { status: json!({"type": "active"}), waiting_for_user: Some(false) },
        SessionChange::Status { status: Value::Null, waiting_for_user: Some(true) },
        SessionChange::Metadata { metadata: json!([null, true, 1, "metadata"]) },
        SessionChange::Lifecycle { state: "loaded".into() },
        SessionChange::Goal { goal: Value::Null },
        SessionChange::Hook { run: json!({"id": "hook-1"}) },
        SessionChange::ApprovalReview { review: json!({"status": "running"}), complete: false },
        SessionChange::ApprovalReview { review: Value::Null, complete: true },
        SessionChange::StrictReview { notice: json!("review") },
        SessionChange::Safety { buffering: json!({"showBufferingUi": true}) },
        SessionChange::Safety { buffering: json!({"showBufferingUi": false}) },
        SessionChange::Safety { buffering: json!({"showBufferingUi": "true"}) },
        SessionChange::Safety { buffering: Value::Null },
        SessionChange::Rerouted { from: "model-a".into(), to: "model-b".into(), reason: "capacity".into() },
        SessionChange::Verification { verifications: vec![json!({"status": "done"}), Value::Null] },
        SessionChange::Error { error: json!({"message": "retrying"}), will_retry: true },
        SessionChange::Error { error: json!({"message": ""}), will_retry: false },
        SessionChange::Error { error: json!({"message": 7}), will_retry: false },
        SessionChange::Error { error: Value::Null, will_retry: false },
        SessionChange::Warning { text: "warning".into() },
        SessionChange::Settings { settings: json!({"model": "model-a"}) },
        SessionChange::Environment { environment: json!({"name": "desktop"}), connected: true },
        SessionChange::Environment { environment: Value::Null, connected: false },
        SessionChange::Queue,
        SessionChange::ToolServer {
            server: "tools".into(),
            status: "failed".into(),
            error: json!("unavailable"),
            failure_reason: json!({"kind": "startup"}),
        },
        SessionChange::ToolServer {
            server: "tools".into(),
            status: "failed".into(),
            error: json!({"message": "unavailable"}),
            failure_reason: Value::Null,
        },
        SessionChange::ToolServer {
            server: "tools".into(),
            status: "ready".into(),
            error: Value::Null,
            failure_reason: Value::Null,
        },
        // Defaults affect reading old data, but the normalized output still has failure_reason.
        serde_json::from_value(json!({"kind": "tool_server", "server": "tools", "status": "failed", "error": null}))
            .unwrap(),
        SessionChange::Attachment { attachment: json!({"path": "diagram.png"}) },
        SessionChange::Request { id: json!(42), request: "approve".into(), params: json!({"command": "pwd"}) },
        SessionChange::Request {
            id: json!({"server":"123:2026-10-04T12:00:00Z","id":0}),
            request: "item/fileChange/requestApproval".into(),
            params: json!({"threadId":"thread","itemId":"item"}),
        },
        SessionChange::Request { id: json!("request-1"), request: "input".into(), params: Value::Null },
        SessionChange::Resolved { id: json!("request-1"), resolution: json!(true) },
        SessionChange::Resolved { id: Value::Null, resolution: Value::Null },
        SessionChange::Voice { event: "started".into(), data: json!({"sessionId": "voice-1"}) },
        SessionChange::AgentStarted { agent: Box::new(HostAgentStart { id: "agent-1".into(), ..Default::default() }) },
        SessionChange::AgentStarted {
            agent: Box::new(HostAgentStart {
                id: "agent-2".into(),
                call: Some("toolu-1".into()),
                parent: Some("agent-1".into()),
                kind: Some("Explore".into()),
                name: Some("scout".into()),
                task: Some("Find the callers".into()),
                prompt: Some("Find every caller of fold().".into()),
                model: Some("claude-haiku-5-5".into()),
                background: true,
                live: true,
            }),
        },
        SessionChange::AgentEvent {
            agent_id: "agent-2".into(),
            happened: Box::new(Happened::TurnStarted { turn: "agent-turn".into() }),
        },
        SessionChange::AgentEnded {
            agent_id: "agent-2".into(),
            end: HostAgentEnd {
                how: TurnHow::Completed,
                reason: None,
                duration_ms: Some(61_000),
                tokens: Some(41_200),
                tools: Some(23),
                answer: Some("Three callers.".into()),
                task: Some("Find the callers".into()),
                told: true,
            },
        },
        SessionChange::AgentEnded {
            agent_id: "agent-1".into(),
            end: HostAgentEnd {
                how: TurnHow::Failed,
                reason: Some("errored".into()),
                duration_ms: None,
                tokens: None,
                tools: None,
                answer: None,
                task: None,
                told: false,
            },
        },
    ]
}

fn tools() -> Vec<Tool> {
    let edits = [
        ChangeHow::Added,
        ChangeHow::Deleted,
        ChangeHow::Updated,
        ChangeHow::Moved { to: "new.txt".into() },
        ChangeHow::Replaced,
    ]
    .into_iter()
    .map(|how| Change { path: "old.txt".into(), how, diff: "-old\n+new\n".into() })
    .collect();
    let does = vec![
        Does::Command { command: "pwd".into(), cwd: None, exit: None, job: None },
        Does::Command {
            command: "exit 1".into(),
            cwd: Some("/work".into()),
            exit: Some(-1),
            job: Some("j-abc123".into()),
        },
        Does::Edit { changes: edits },
        Does::Search { query: "serde JSON".into() },
        Does::Image { path: None },
        Does::Image { path: Some("diagram.png".into()) },
        Does::Colleague { to: None, task: None },
        Does::Colleague { to: Some("codex-ab12".into()), task: Some("inspect".into()) },
        Does::Agent {
            act: AgentAct::Start,
            agents: vec![],
            kind: Some("Explore".into()),
            task: Some("Find callers".into()),
        },
        Does::Agent { act: AgentAct::Wait, agents: vec!["thread-1".into(), "thread-2".into()], kind: None, task: None },
        Does::Agent { act: AgentAct::Message, agents: vec!["thread-1".into()], kind: None, task: Some("go on".into()) },
        Does::Agent { act: AgentAct::Resume, agents: vec![], kind: None, task: None },
        Does::Agent { act: AgentAct::Stop, agents: vec![], kind: None, task: None },
        Does::Agent { act: AgentAct::List, agents: vec![], kind: None, task: None },
        Does::Call,
    ];
    let statuses = [
        ToolStatus::Running,
        ToolStatus::Done,
        ToolStatus::Failed,
        ToolStatus::Declined,
        ToolStatus::Cut,
        ToolStatus::Unknown,
    ];
    does.into_iter()
        .enumerate()
        .map(|(i, does)| Tool {
            run: (i == 1).then(|| Box::new(super::run_facts::example())),
            name: format!("tool-{i}"),
            server: (i != 0).then(|| "tools".into()),
            machine: (i != 0).then(|| "desktop".parse().unwrap()),
            does,
            input: if i == 0 { Value::Null } else { json!({"args": [null, false, 1, "text", {}]}) },
            output: if i == 0 {
                vec![]
            } else {
                vec![
                    Content::Text { text: "output".into() },
                    Content::Image { mime: "image/png".into(), base64: "aW1hZ2U=".into() },
                    Content::ImageUrl { url: "https://example.test/image.png".into() },
                    Content::ImageRef {
                        mime: "image/png".into(),
                        content: fastctx_types::session::Fingerprint { size: 5, sha256: "1".repeat(64) },
                        source: Box::new(fastctx_types::attachments::AttachmentSource::ToolImage {
                            owner: address(),
                            tool_image: fastctx_types::attachments::ToolImage {
                                turn: Some("turn-1".into()),
                                item: "tool-1".into(),
                                image: 0,
                            },
                        }),
                    },
                    Content::AudioUrl { url: "https://example.test/audio.wav".into() },
                    Content::Other { block: json!({"type": "resource_link", "uri": "file:///example.txt"}) },
                    Content::Other { block: Value::Null },
                ]
            },
            status: statuses[i % statuses.len()],
            duration_ms: (i != 0).then_some(42),
        })
        .collect()
}

pub fn fixtures() -> Fixtures {
    let turn = "turn-1".to_string();
    let mut events = vec![
        event(Happened::TurnStarted { turn: turn.clone() }),
        // Exercise the default for delivery without inventing an optional output field.
        serde_json::from_value(
            json!({"at": "1970-01-01T00:00:00Z", "event": "heard", "turn": null, "from": null, "text": "old record"}),
        )
        .unwrap(),
    ];
    for (i, from) in [
        Speaker::User { machine: "desktop".parse().unwrap() },
        Speaker::Resident { address: address() },
        Speaker::World,
    ]
    .into_iter()
    .enumerate()
    {
        let message = if i == 0 {
            Some(Box::new(UserMessage {
                attachments: Vec::new(),
                images: vec![fastctx_types::session::images::UserImage {
                    source: fastctx_types::attachments::AttachmentSource::HostImage {
                        owner: address(),
                        host_image: fastctx_types::attachments::HostImage::Codex {
                            turn: turn.clone(),
                            message: 0,
                            image: 0,
                        },
                    },
                    name: Some("image.png".into()),
                    media_type: Some("image/png".into()),
                    content: Some(fastctx_types::session::Fingerprint { size: 68, sha256: "0".repeat(64) }),
                }],
                id: "message-1".into(),
                client_id: Some("client-1".into()),
                content: vec![
                    json!({"type": "text", "text": "hello 世界"}),
                    json!({"type": "image", "url": "image.png"}),
                ],
                hook_fragments: vec![json!({"source": "hook"})],
                presentation: json!({"mentions": []}),
                detail: json!({"source": "host"}),
            }))
        } else if i == 1 {
            Some(Box::new(
                serde_json::from_value(
                    json!({"id": "message-2", "client_id": null, "content": [], "hook_fragments": []}),
                )
                .unwrap(),
            ))
        } else {
            None
        };
        events.push(event(Happened::Heard {
            delivery: Some("desktop-notice-1".parse().unwrap()),
            turn: Some(turn.clone()),
            from: Some(from),
            text: "hello 世界".into(),
            message,
            notice: match i {
                2 => Some(Box::new(fastctx_types::residents::Notice::World {
                    fact: fastctx_types::residents::WorldFact::Moved { to: Some("mark50".parse().unwrap()) },
                    text: "=== the user moved you to the project mark50 ===".into(),
                })),
                1 => Some(Box::new(fastctx_types::residents::Notice::World {
                    fact: fastctx_types::residents::WorldFact::Focus {
                        focus: fastctx_types::focus::Focus {
                            machines: vec![fastctx_types::focus::FocusedMachine {
                                machine: "laptop".parse().unwrap(),
                                folders: vec!["C:/work/app".into()],
                            }],
                        },
                    },
                    text: "The user changed the machines and folders of this conversation.".into(),
                })),
                _ => None,
            },
        }));
    }
    for (id, host_image, name, media_type, content) in [
        (
            "native-codex-provider",
            fastctx_types::attachments::HostImage::Codex { turn: turn.clone(), message: 1, image: 2 },
            None,
            None,
            json!({"type":"image","fileId":"provider-reference"}),
        ),
        (
            "native-claude-user",
            fastctx_types::attachments::HostImage::Claude { message: "native-claude-user".into(), image: 0 },
            None,
            Some("image/png".to_owned()),
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}),
        ),
    ] {
        events.push(event(Happened::Heard {
            delivery: None,
            turn: Some(turn.clone()),
            from: None,
            text: String::new(),
            notice: None,
            message: Some(Box::new(UserMessage {
                attachments: Vec::new(),
                images: vec![fastctx_types::session::images::UserImage {
                    source: fastctx_types::attachments::AttachmentSource::HostImage {
                        owner: match &host_image {
                            fastctx_types::attachments::HostImage::Claude { .. } => {
                                "claude-ab12@desktop".parse().unwrap()
                            }
                            _ => address(),
                        },
                        host_image,
                    },
                    name,
                    media_type,
                    content: None,
                }],
                id: id.into(),
                client_id: None,
                content: vec![content],
                hook_fragments: Vec::new(),
                presentation: Value::Null,
                detail: Value::Null,
            })),
        }));
    }
    events.push(event(Happened::Heard {
        delivery: Some("goal-question-1".parse().unwrap()),
        turn: Some(turn.clone()),
        from: Some(Speaker::Resident { address: address() }),
        text: "Question from a colleague: which output format?".into(),
        message: None,
        notice: Some(Box::new(fastctx_types::residents::Notice::Words {
            from: Speaker::Resident { address: address() },
            text: "Question from a colleague: which output format?".into(),
            goal_report: None,
            goal_question: Some(Box::new(fastctx_types::goals::QuestionDelivery {
                id: "goal-question-1".into(),
                colleague: address(),
                account: Some(fastctx_types::access::HubAccount {
                    hub: "w-0123456789abcdef".parse().unwrap(),
                    account: "ada".parse().unwrap(),
                }),
                questions: Some(json!([{"question":"Which output format?","header":"Format","multiSelect":false,
                    "options":[{"label":"Text","description":"Plain lines"},{"label":"Table"}]}])),
            })),
        })),
    }));
    events.push(event(Happened::Heard {
        delivery: Some("goal-result-1".parse().unwrap()),
        turn: Some(turn.clone()),
        from: Some(Speaker::Resident { address: address() }),
        text: "=== colleague codex-ab12 on desktop (goal complete, 2h14m) ===\nOriginal report.\n=== what it did: fastctx w codex-ab12 ===".into(),
        message: None,
        notice: Some(Box::new(fastctx_types::residents::Notice::Words {
            from: Speaker::Resident { address: address() },
            text: "=== colleague codex-ab12 on desktop (goal complete, 2h14m) ===\nOriginal report.\n=== what it did: fastctx w codex-ab12 ===".into(),
            goal_question: None,
            goal_report: Some(Box::new(fastctx_types::goals::ReportDelivery {
                id: "goal-result-1".parse().unwrap(), completion: Some("goal-completion-1".parse().unwrap()), colleague: address(),
                account: fastctx_types::access::HubAccount { hub: "w-0123456789abcdef".parse().unwrap(), account: "ada".parse().unwrap() },
                shown: Some(fastctx_types::goals::ReportShown {
                    label: "fit the model".into(),
                    status: fastctx_types::goals::Status::Complete,
                    started_at: "2026-10-23T08:00:00Z".parse().unwrap(),
                    ended_at: "2026-10-23T10:14:00Z".parse().unwrap(),
                    resume_at: None,
                    text: "Original report.".into(),
                }),
            })),
        })),
    }));
    let mut question = item("goal-question-card", What::Message { text: "Which output format?".into() });
    question.detail = Some(json!({
        "delivery":"async",
        "questions":[{"title":"Which output format?","options":["Text","Table"],"multiSelect":false}],
        "fastctx_goal_question":{"tool_use_id":"question-tool","agent_id":null,
            "questions":[{"question":"Which output format?","options":[{"label":"Text"},{"label":"Table"}]}]}
    }));
    events.push(event(Happened::Finished { turn: turn.clone(), item: question }));
    let mut items = vec![
        item("message", What::Message { text: "hello".into() }),
        item("thinking", What::Thinking { text: "thinking".into(), duration_ms: Some(4_200) }),
        item("plan", What::Plan { text: "plan".into() }),
        item("compaction", What::Compaction),
        item(
            "activity",
            What::AgentActivity { agent: "codex".into(), path: "/root/check".into(), activity: "reading".into() },
        ),
        item("hidden", What::Hidden { host_kind: "providerHidden".into() }),
        item("other", What::Other { host_kind: "providerFutureItem".into() }),
    ];
    items.extend(
        tools().into_iter().enumerate().map(|(i, tool)| item(&format!("tool-{i}"), What::Tool(Box::new(tool)))),
    );
    for (i, mut item) in items.into_iter().enumerate() {
        if i % 2 == 0 {
            item.started_at = Some("2026-10-04T12:06:07.120Z".parse().unwrap());
            item.detail = Some(if i == 0 { Value::Null } else { json!({"source": "host"}) });
        }
        events.push(event(Happened::Began { turn: turn.clone(), item: item.clone() }));
        events.push(event(Happened::Updated { turn: turn.clone(), item: item.clone() }));
        events.push(event(Happened::Finished { turn: turn.clone(), item }));
    }
    // A reply kept under its host's own name after it streamed as a draft.
    let mut kept = item("[\"row-uuid\",1]", What::Message { text: "kept".into() });
    kept.draft = Some("[\"turn-1\",0,1,\"text\"]".into());
    events.push(event(Happened::Finished { turn: turn.clone(), item: kept }));
    for (part, revision) in [(Part::Text, None), (Part::Output, Some(0)), (Part::Output, Some(u64::MAX))] {
        events.push(event(Happened::Grew {
            turn: turn.clone(),
            item: "tool-0".into(),
            part,
            revision,
            text: "more".into(),
        }));
    }
    let health_cases: Vec<_> =
        changes().into_iter().map(|change| HealthCase { health: change.health(), change }).collect();
    for (i, case) in health_cases.iter().enumerate() {
        events.push(event(Happened::Changed { turn: (i % 2 == 0).then(|| turn.clone()), change: case.change.clone() }));
    }
    for complete in [false, true] {
        events.push(event(Happened::Realtime {
            item: json!({"id": "voice-item", "realtimeSessionId": "voice-session"}),
            complete,
        }));
    }
    events.push(event(Happened::RealtimeGrew { item: "voice-item".into(), text: "voice text".into() }));
    events.push(event(Happened::Steps {
        turn: turn.clone(),
        note: None,
        steps: vec![
            Step { text: "first".into(), status: StepStatus::Pending, id: None, active_form: None, detail: None },
            Step {
                text: "second".into(),
                status: StepStatus::Doing,
                id: Some("step-2".into()),
                active_form: Some("checking".into()),
                detail: Some(Value::Null),
            },
            Step {
                text: "third".into(),
                status: StepStatus::Done,
                id: Some("step-3".into()),
                active_form: Some("checked".into()),
                detail: Some(json!({"owner": "host"})),
            },
        ],
    }));
    events.push(event(Happened::Steps { turn: turn.clone(), note: Some("no steps".into()), steps: vec![] }));
    events.push(event(Happened::Context { used: 42, window: Some(1000) }));
    events.push(event(Happened::Context { used: u64::MAX, window: None }));
    events.push(event(Happened::Titled { title: "Wire contract".into() }));
    for (i, how) in [TurnHow::Completed, TurnHow::Failed, TurnHow::Interrupted, TurnHow::Other("provider_end".into())]
        .into_iter()
        .enumerate()
    {
        events.push(event(Happened::TurnEnded {
            turn: format!("turn-{i}"),
            end: TurnEnd {
                facts: match i {
                    0 => Some(Box::new(TurnFacts {
                        world_started_at: None,
                        world_last_activity_at: None,
                        world_progress: false,
                        world_compacted: false,
                        models: vec![],
                        efforts: vec![],
                        tokens: Some(TurnTokens::Codex {
                            input_tokens: u64::MAX,
                            cached_input_tokens: 0,
                            cache_write_input_tokens: None,
                            output_tokens: 0,
                            reasoning_output_tokens: 0,
                            total_tokens: u64::MAX,
                        }),
                        cost: None,
                        limits: Some(fastctx_types::agents::AgentLimits {
                            observed_at: "2026-10-12T14:20:00Z".parse().unwrap(),
                            windows: vec![fastctx_types::agents::LimitWindow {
                                name: "codex".into(),
                                minutes: Some(10_080),
                                used_tenths: 90,
                                resets_at: Some("2026-10-15T18:00:00Z".parse().unwrap()),
                                since: None,
                            }],
                            credits: None,
                        }),
                    })),
                    1 => Some(Box::new(TurnFacts {
                        world_started_at: None,
                        world_last_activity_at: None,
                        world_progress: false,
                        world_compacted: false,
                        models: vec!["response-model-a".into(), "response-model-b".into()],
                        efforts: vec![
                            TurnEffort::Requested { value: json!("low") },
                            TurnEffort::Requested { value: json!(42) },
                        ],
                        tokens: Some(TurnTokens::Claude {
                            input_tokens: 15,
                            cache_read_input_tokens: 20,
                            cache_creation_input_tokens: 30,
                            output_tokens: 40,
                        }),
                        cost: Some(fastctx_types::usage::Usd(421_337)),
                        limits: None,
                    })),
                    _ => None,
                },
                how,
                reason: (i != 0).then(|| "reason".into()),
                duration_ms: (i != 0).then_some(0),
                last_message: (i != 0).then(|| "last message".into()),
                error: match i {
                    0 => None,
                    1 => Some(Value::Null),
                    _ => Some(json!({"code": "provider_error"})),
                },
            },
        }));
    }
    for kind in [
        HealthKind::Reconnecting,
        HealthKind::Safety,
        HealthKind::Rerouted,
        HealthKind::ToolsMissing,
        HealthKind::ToolServer,
        HealthKind::Warning,
        HealthKind::Error,
    ] {
        events.push(event(Happened::Health { turn: None, health: Health { kind, text: "host notice".into() } }));
    }
    events.push(event(Happened::Health {
        turn: Some(turn),
        health: Health { kind: HealthKind::Warning, text: "turn notice".into() },
    }));
    events.push(event(Happened::DeliveryUncertain {
        delivery: "desktop-notice-1".parse().unwrap(),
        why: "host disconnected".into(),
    }));
    let watched = vec![
        Watched::History { events: vec![] },
        Watched::History { events: vec![events[0].clone()] },
        Watched::Live { event: events[1].clone() },
        Watched::Unchanged { version: "version-1".into() },
        Watched::Segments {
            version: "version-2".into(),
            layout: vec![fastctx_types::session::held::SegmentId::of(&[7; 32])],
            segments: vec![fastctx_types::session::held::Segment {
                id: fastctx_types::session::held::SegmentId::of(&[7; 32]),
                events: vec![events[0].clone()],
            }],
            complete: true,
        },
    ];
    Fixtures { events, watched, health_cases }
}
