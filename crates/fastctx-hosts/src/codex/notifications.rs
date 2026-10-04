//! Thread presentation state. These updates do not create tool/message items.

use fastctx_types::session::{Happened, SessionChange};
use serde_json::Value;

pub fn read(method: &str, params: &Value) -> Option<Happened> {
    let value = |key: &str| params.get(key).cloned().unwrap_or(Value::Null);
    let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let change = match method {
        "thread/reverted" => SessionChange::HistoryChanged,
        "thread/started" => SessionChange::Metadata { metadata: value("thread") },
        "thread/status/changed" => {
            let status = value("status");
            let waiting_for_user = match status.get("type").and_then(Value::as_str) {
                Some("active") => status.get("activeFlags").and_then(Value::as_array).map(|flags| {
                    flags.iter().any(|flag| matches!(flag.as_str(), Some("waitingOnApproval" | "waitingOnUserInput")))
                }),
                Some("idle" | "notLoaded" | "systemError") => Some(false),
                _ => None,
            };
            SessionChange::Status { status, waiting_for_user }
        }
        "thread/archived" => SessionChange::Lifecycle { state: "archived".into() },
        "thread/unarchived" => SessionChange::Lifecycle { state: "unarchived".into() },
        "thread/deleted" => SessionChange::Lifecycle { state: "deleted".into() },
        "thread/settings/updated" => SessionChange::Settings { settings: params.clone() },
        "thread/environment/connected" | "thread/environment/disconnected" => SessionChange::Environment {
            environment: params.clone(),
            connected: method == "thread/environment/connected",
        },
        "thread/queue/changed" => SessionChange::Queue,
        "thread/attachment/updated" => SessionChange::Attachment { attachment: params.clone() },
        "thread/goal/updated" => SessionChange::Goal { goal: value("goal") },
        "thread/goal/cleared" => SessionChange::Goal { goal: Value::Null },
        "turn/diff/updated" => SessionChange::Diff { diff: text("diff") },
        "item/commandExecution/terminalInteraction" => {
            SessionChange::Terminal { item: text("itemId"), process: text("processId"), input: text("stdin") }
        }
        "hook/started" | "hook/completed" => SessionChange::Hook { run: value("run") },
        "item/autoApprovalReview/started" | "item/autoApprovalReview/completed" => {
            SessionChange::ApprovalReview { review: params.clone(), complete: method.ends_with("/completed") }
        }
        "autoApprovalReview/strictReviewRequired" => SessionChange::StrictReview { notice: params.clone() },
        "model/safetyBuffering/updated" => SessionChange::Safety { buffering: params.clone() },
        "model/rerouted" => {
            SessionChange::Rerouted { from: text("fromModel"), to: text("toModel"), reason: text("reason") }
        }
        "model/verification" => SessionChange::Verification {
            verifications: value("verifications")
                .as_array()?
                .iter()
                .filter(|v| v.as_str() == Some("trustedAccessForCyber"))
                .cloned()
                .collect(),
        },
        "guardianWarning"
            if params.get("kind").and_then(Value::as_str) == Some("tooManyDenials")
                || text("message")
                    .starts_with("Automatic approval review rejected too many approval requests for this turn") =>
        {
            SessionChange::Warning { text: text("message") }
        }
        "serverRequest/resolved" => SessionChange::Resolved { id: value("requestId"), resolution: params.clone() },
        "thread/realtime/item/started" | "thread/realtime/item/completed" => {
            return Some(Happened::Realtime { item: value("item"), complete: method.ends_with("/completed") });
        }
        "thread/realtime/item/transcript/delta" => {
            return Some(Happened::RealtimeGrew { item: text("itemId"), text: text("delta") });
        }
        "thread/realtime/started"
        | "thread/realtime/closed"
        | "thread/realtime/error"
        | "thread/realtime/itemAdded"
        | "thread/realtime/transcript/delta"
        | "thread/realtime/transcript/done" => {
            SessionChange::Voice { event: method.strip_prefix("thread/realtime/")?.to_string(), data: params.clone() }
        }
        _ => return None,
    };
    Some(Happened::Changed { turn: params.get("turnId").and_then(Value::as_str).map(str::to_string), change })
}
