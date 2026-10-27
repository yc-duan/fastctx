//! Claude's opaque reply addresses and public request/answer contracts.

use fastctx_types::session::control::{Control, ControlKind, Offers};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
pub struct Fixtures {
    pub events: Vec<SessionEvent>,
    pub answers: Vec<Control>,
    pub unavailable: Offers,
    pub idle: Offers,
}

pub fn fixtures() -> Fixtures {
    let at = "2026-10-04T17:00:00Z".parse().unwrap();
    let cases = [
        (
            "classic.PermissionRequest",
            json!({"session_id":"session-1","hook_event_name":"PermissionRequest","tool_name":"Bash", "tool_input":{"command":"pwd"}, "permission_suggestions":[],"permission_mode":"default"}),
            json!({"decision":{"behavior":"allow"}}),
        ),
        (
            "Elicitation",
            json!({"session_id":"session-1","hook_event_name":"Elicitation","mcp_server_name":"fixture","message":"Choose a city", "mode":"form", "requested_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}),
            json!({"action":"accept","content":{"city":"Hangzhou"}}),
        ),
    ];
    let mut events = Vec::new();
    let mut answers = Vec::new();
    for (index, (request, params, answer)) in cases.into_iter().enumerate() {
        let id = json!(["claude", "session-1", "1791133200000", format!("r-{index}")]);
        events.push(SessionEvent {
            at,
            happened: Happened::Changed {
                turn: Some("turn-1".into()),
                change: SessionChange::Request { id: id.clone(), request: request.into(), params },
            },
        });
        answers.push(Control::Answer { request: id.clone(), answer: answer.clone() });
        events.push(SessionEvent {
            at,
            happened: Happened::Changed {
                turn: Some("turn-1".into()),
                change: SessionChange::Resolved { id, resolution: json!({"kind":"answer","answer":answer}) },
            },
        });
    }
    Fixtures {
        events,
        answers,
        unavailable: Offers::default(),
        idle: Offers {
            controls: vec![
                ControlKind::Command,
                ControlKind::Compact,
                ControlKind::Model,
                ControlKind::Rename,
                ControlKind::Fork,
            ],
            ..Offers::default()
        },
    }
}
