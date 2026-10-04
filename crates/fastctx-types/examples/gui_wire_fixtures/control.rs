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
            Control::Model { model: Some("example-model".into()), effort: Some("custom-effort".into()) },
            Control::Model { model: None, effort: None },
            Control::Rename { name: "对话".into() },
            Control::Fork { through: None },
            Control::Fork { through: Some("turn-1".into()) },
            Control::Rewind { from: "turn-2".into() },
            Control::Answer {
                request: json!({"server":"123:2026-10-04T12:00:00Z","id":0}),
                answer: json!({"decision":"accept"}),
            },
        ],
        controlled: vec![
            Controlled::Done { said: None },
            Controlled::Done { said: Some("Conversation rewound. Files have not been restored.".into()) },
            Controlled::Forked { resident: "codex-ab12@desktop".parse().unwrap() },
        ],
        offers: vec![
            Offers::default(),
            Offers {
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
                        source: Some("codex".into()),
                        path: None,
                    },
                    CommandOffer {
                        name: "skill".into(),
                        description: "A skill".into(),
                        args: Some("[instructions]".into()),
                        source: Some("skill".into()),
                        path: Some("C:/skills/example/SKILL.md".into()),
                    },
                ],
                models: vec![ModelOffer {
                    id: "model".into(),
                    name: Some("Model".into()),
                    description: None,
                    efforts: vec!["custom-effort".into()],
                    default_effort: Some("custom-effort".into()),
                    default: true,
                }],
                model: Some("model".into()),
                effort: Some("custom-effort".into()),
                usage: Some(Usage {
                    context: Some(ContextUsage { used: 42, window: Some(1000) }),
                    limits: vec![Limit {
                        name: "codex primary 300m".into(),
                        used_percent: Some(12.into()),
                        resets_at: Some("2026-10-05T00:00:00Z".parse().unwrap()),
                    }],
                    cost_usd: None,
                    raw: json!({"accountRateLimits":{"error":"usage unavailable"}}),
                }),
            },
        ],
    }
}
