use fastctx_types::session::control::*;
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
pub struct Fixtures {
    controls: Vec<Control>,
    controlled: Vec<Controlled>,
    offers: Vec<Offers>,
}

pub fn fixtures() -> Fixtures {
    Fixtures {
        controls: vec![
            Control::Command { name: "review".into(), args: "Check this change".into(), path: None },
            Control::Command {
                name: "skill".into(),
                args: "Use it".into(),
                path: Some("C:/skills/example/SKILL.md".into()),
            },
            Control::Compact,
            Control::CancelControl { id: "control-1".into() },
            Control::Model { model: Some("example-model".into()), effort: Some("custom-effort".into()) },
            Control::Model { model: None, effort: None },
            Control::Rename { name: "对话".into() },
            Control::Fork { through: None, side: false },
            Control::Fork { through: Some("turn-1".into()), side: false },
            Control::Fork { through: None, side: true },
            Control::Rewind { from: "turn-2".into() },
            Control::Answer {
                request: json!({"server":"123:2026-10-04T12:00:00Z","id":0}),
                answer: json!({"decision":"accept"}),
            },
        ],
        controlled: vec![
            Controlled::Done { said: None },
            Controlled::SideAnswer { reply: json!({"isAnswered":true,"text":"Side answer"}) },
            Controlled::Done { said: Some("Conversation rewound. Files have not been restored.".into()) },
            Controlled::Forked { resident: "codex-ab12@desktop".parse().unwrap() },
            Controlled::Reported { said: Some("Host output without a success flag".into()), queued: None },
            Controlled::Reported {
                said: Some("Waiting for the current turn to end.".into()),
                queued: Some("control-1".into()),
            },
            Controlled::Refused { said: "The host refused this change".into(), current_model: Some("model".into()) },
            Controlled::ModelMenu { models: Vec::new(), model: None, effort: None, service_tier: None },
        ],
        offers: vec![
            Offers::default(),
            Offers {
                pending_controls: vec![PendingControl {
                    id: "control-1".into(),
                    control: Control::Compact,
                    at: "2026-10-21T00:00:00Z".parse().unwrap(),
                    state: ControlState::Waiting,
                    result: None,
                    error: None,
                }],
                control_queue: true,
                controls_told: true,
                control_reply_envelope: false,
                checkpoints: None,
                history_limits: None,
                automatic_model: None,
                controls: vec![
                    ControlKind::Command,
                    ControlKind::Compact,
                    ControlKind::Model,
                    ControlKind::Rename,
                    ControlKind::Fork,
                    ControlKind::Rewind,
                    ControlKind::Answer,
                ],
                commands: vec![
                    CommandOffer {
                        name: "compact".into(),
                        description: "Compact".into(),
                        args: None,
                        argument_forms: Vec::new(),
                        arguments: CommandArguments::None,
                        source: Some("codex".into()),
                        path: None,
                    },
                    CommandOffer {
                        name: "skill".into(),
                        description: "A skill".into(),
                        args: Some("[instructions]".into()),
                        argument_forms: Vec::new(),
                        arguments: CommandArguments::Optional,
                        source: Some("skill".into()),
                        path: Some("C:/skills/example/SKILL.md".into()),
                    },
                    CommandOffer {
                        name: "rename".into(),
                        description: "Rename".into(),
                        args: Some("<name>".into()),
                        argument_forms: Vec::new(),
                        arguments: CommandArguments::Required,
                        source: Some("builtin".into()),
                        path: None,
                    },
                ],
                models: vec![ModelOffer {
                    id: "model".into(),
                    name: Some("Model".into()),
                    description: None,
                    efforts: vec!["custom-effort".into()],
                    default_effort: Some("custom-effort".into()),
                    default: true,
                    service_tiers: vec![ServiceTierOffer {
                        id: "priority".into(),
                        name: "Fast".into(),
                        description: "Faster responses".into(),
                    }],
                    default_service_tier: None,
                    context_window: Some(1_000_000),
                }],
                model: Some("model".into()),
                effort: Some("custom-effort".into()),
                service_tier: Some("priority".into()),
                usage: Some(Usage {
                    context: Some(ContextUsage { used: 42, window: Some(1000) }),
                    limits: vec![Limit {
                        name: "codex primary 300m".into(),
                        used_percent: Some(12.into()),
                        resets_at: Some("2026-10-05T00:00:00Z".parse().unwrap()),
                    }],
                    limits_at: Some("2026-10-04T23:50:00Z".parse().unwrap()),
                    cost_usd: None,
                    raw: json!({"accountRateLimits":{"error":"usage unavailable"}}),
                }),
            },
        ],
    }
}
