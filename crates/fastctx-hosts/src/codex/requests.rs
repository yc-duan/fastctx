//! App-server observations approximate request timing; they never contain message content.
use fastctx_types::usage_calls::{CallOutcome, CallPhase, CallTiming};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Requests {
    turn: Option<String>,
    started: Option<Timestamp>,
    first: Option<Timestamp>,
    pending: bool,
    #[serde(default)]
    sequence: u64,
}

#[derive(Serialize)]
pub(super) struct Observation {
    pub sequence: u64,
    pub timing: CallTiming,
    pub outcome: CallOutcome,
    pub phase: CallPhase,
}
impl Requests {
    pub fn observe(&mut self, method: &str, params: &Value, at: Timestamp) -> Option<Observation> {
        if method == "turn/started" {
            let turn = params.pointer("/turn/id").and_then(Value::as_str).map(str::to_owned);
            if self.turn != turn {
                self.turn = turn;
                self.started = Some(at);
                self.first = None;
                self.pending = true;
            }
        }
        if method == "item/started"
            && matches!(params.pointer("/item/type").and_then(Value::as_str), Some("agentMessage" | "reasoning"))
        {
            self.pending = true;
            self.started.get_or_insert(at);
        }
        if method.ends_with("/delta")
            && (method.starts_with("item/agentMessage/") || method.starts_with("item/reasoning/"))
        {
            self.first.get_or_insert(at);
        }
        if method == "turn/completed" {
            let outcome = match params.pointer("/turn/status").and_then(Value::as_str) {
                Some("failed") => Some(CallOutcome::Failed),
                Some("interrupted") => Some(CallOutcome::Interrupted),
                _ => None,
            };
            if (self.pending || response_failure(params.pointer("/turn/error/codexErrorInfo")))
                && let Some(outcome) = outcome
            {
                return Some(self.finish(at, outcome));
            }
            self.pending = false;
        }
        None
    }

    pub fn finish(&mut self, at: Timestamp, outcome: CallOutcome) -> Observation {
        let phase = if outcome == CallOutcome::Completed {
            CallPhase::Finished
        } else if self.first.is_some() {
            CallPhase::Streaming
        } else {
            CallPhase::BeforeResponse
        };
        let timing = CallTiming { started_at: self.started, first_at: self.first, ended_at: at, approximate: true };
        // The next request may include intervening tool work; the app-server has no send boundary.
        self.started = Some(at);
        self.first = None;
        self.pending = false;
        self.sequence += 1;
        Observation { sequence: self.sequence, timing, outcome, phase }
    }
}

// Structured provider failures also identify a request that produced no item after a tool call.
fn response_failure(info: Option<&Value>) -> bool {
    let Some(info) = info else { return false };
    let kind = info.as_str().or_else(|| info.as_object().and_then(|o| o.keys().next()).map(String::as_str));
    matches!(
        kind,
        Some(
            "httpConnectionFailed"
                | "responseStreamConnectionFailed"
                | "responseStreamDisconnected"
                | "responseTooManyFailedAttempts"
                | "serverOverloaded"
                | "usageLimitExceeded"
                | "unauthorized"
                | "badRequest"
                | "internalServerError"
                | "contextWindowExceeded"
        )
    )
}
