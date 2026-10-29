//! Validate before replying: 0.160 consumes even malformed responses. The
//! frozen response schemas were exported by the official 0.160 binary.

use std::sync::Arc;

use serde_json::{Value, json};

use super::CodexHost;

pub(super) struct Request {
    pub at: jiff::Timestamp,
    pub id: Value,
    pub method: String,
    pub params: Value,
}

impl Request {
    pub(super) fn supported(&self) -> bool {
        schema(&self.method).is_some()
            && (self.method != "mcpServer/elicitation/request"
                || matches!(self.params["mode"].as_str(), Some("form" | "url" | "openai/form" | "openaiForm")))
    }

    fn validate(&self, answer: &Value) -> Result<(), String> {
        if !self.supported() {
            return Err(format!(
                "Fastctx does not answer {} in this mode through a stable 0.160 interface",
                self.method
            ));
        }
        let schema: Value = serde_json::from_str(schema(&self.method).expect("supported response"))
            .map_err(|error| format!("reading the frozen response contract: {error}"))?;
        validate(&schema, answer)?;
        match self.method.as_str() {
            "item/commandExecution/requestApproval" => {
                let decisions = match self.params.get("availableDecisions").filter(|v| !v.is_null()) {
                    Some(value) => value.as_array().ok_or("Codex supplied invalid availableDecisions")?.clone(),
                    None => legacy_choices(&self.params),
                };
                if !decisions.contains(&answer["decision"]) {
                    return Err("that decision was not offered for this command approval".into());
                }
            }
            "mcpServer/elicitation/request" => {
                if answer["action"] == "accept" {
                    match self.params["mode"].as_str() {
                        Some("url") => {
                            if !answer["content"].is_null() {
                                return Err("URL elicitation accepts no form content".into());
                            }
                        }
                        _ => validate(&self.params["requestedSchema"], &answer["content"])?,
                    }
                } else if !answer["content"].is_null() {
                    return Err("declined or cancelled elicitation must not contain form content".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn validate(schema: &Value, answer: &Value) -> Result<(), String> {
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| format!("invalid response schema: {error}"))?;
    validator.validate(answer).map_err(|error| format!("invalid answer at {}: {error}", error.instance_path))
}

fn schema(method: &str) -> Option<&'static str> {
    Some(match method {
        "item/commandExecution/requestApproval" => {
            include_str!("fixtures/answers-0.160/CommandExecutionRequestApprovalResponse.json")
        }
        "item/fileChange/requestApproval" => {
            include_str!("fixtures/answers-0.160/FileChangeRequestApprovalResponse.json")
        }
        "item/permissions/requestApproval" => {
            include_str!("fixtures/answers-0.160/PermissionsRequestApprovalResponse.json")
        }
        "mcpServer/elicitation/request" => {
            include_str!("fixtures/answers-0.160/McpServerElicitationRequestResponse.json")
        }
        "applyPatchApproval" => include_str!("fixtures/answers-0.160/ApplyPatchApprovalResponse.json"),
        "execCommandApproval" => include_str!("fixtures/answers-0.160/ExecCommandApprovalResponse.json"),
        _ => return None,
    })
}

fn legacy_choices(params: &Value) -> Vec<Value> {
    let mut decisions = vec![json!("accept")];
    if !params["networkApprovalContext"].is_null() {
        decisions.push(json!("acceptForSession"));
        if let Some(amendment) = params["proposedNetworkPolicyAmendments"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["action"] == "allow"))
        {
            decisions.push(json!({"applyNetworkPolicyAmendment":{"network_policy_amendment":amendment}}));
        }
    } else if params["additionalPermissions"].is_null()
        && let Some(amendment) = params.get("proposedExecpolicyAmendment").filter(|value| !value.is_null())
    {
        decisions.push(json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":amendment}}));
    }
    decisions.push(json!("cancel"));
    decisions
}

impl CodexHost {
    pub(super) fn request_address(&self, id: &Value) -> Value {
        match &self.lock().server {
            Some((server, _)) => server.request_key(id),
            None => id.clone(),
        }
    }

    pub(super) fn resolve_requests(&self, state: &mut super::State, reason: &str) {
        use fastctx_types::session::{Happened, SessionChange};
        for (thread, entry) in &mut state.threads {
            for (key, _) in entry.requests.drain() {
                self.emit(
                    thread,
                    jiff::Timestamp::now(),
                    Happened::Changed {
                        turn: None,
                        change: SessionChange::Resolved {
                            id: serde_json::from_str(&key).expect("a serialized reply address"),
                            resolution: json!({"reason":reason}),
                        },
                    },
                );
            }
        }
    }

    /// Current unanswered requests, using the same addresses and timestamps
    /// as live delivery. Subscribe before taking this snapshot.
    pub fn requests(&self, thread: &str) -> Vec<fastctx_types::session::SessionEvent> {
        use fastctx_types::session::{Happened, SessionChange, SessionEvent};
        let mut state = self.lock();
        let Some((server, _)) = state.server.clone() else { return Vec::new() };
        let Some(entry) = state.threads.get(thread) else { return Vec::new() };
        let mut requests: Vec<_> = entry
            .requests
            .values()
            .map(|request| SessionEvent {
                at: request.at,
                happened: Happened::Changed {
                    turn: request.params.get("turnId").and_then(Value::as_str).map(str::to_string),
                    change: SessionChange::Request {
                        id: server.request_key(&request.id),
                        request: request.method.clone(),
                        params: request.params.clone(),
                    },
                },
            })
            .collect();
        requests.sort_by_key(|event| event.at);
        requests
    }

    pub fn request_pending(&self, thread: &str, key: &Value) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry| entry.requests.contains_key(&key.to_string()))
    }

    /// Sends a validated response once on the same connection that heard
    /// the request. Another client can win; JSON-RPC responses have no ack.
    pub fn answer(&self, thread: &str, key: &Value, answer: &Value) -> Result<(), String> {
        let _generation = self.generation.lock().expect("never poisoned");
        let mut state = self.lock();
        let server = state
            .server
            .as_ref()
            .map(|(server, _)| Arc::clone(server))
            .ok_or("the app-server that asked this question is no longer connected")?;
        let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
        let request = entry
            .requests
            .get(&key.to_string())
            .ok_or("this request is already answered, resolved, or belongs to another session/server")?;
        request.validate(answer)?;
        server.answer(&request.id, answer)?;
        let turn = request.params.get("turnId").and_then(Value::as_str).map(str::to_string);
        entry.requests.remove(&key.to_string());
        // Every World view must stop offering a second response even if
        // the host disconnects before acknowledging resolution. This is a
        // submission fact, never a claim that this client won the race.
        self.emit(
            thread,
            jiff::Timestamp::now(),
            fastctx_types::session::Happened::Changed {
                turn,
                change: fastctx_types::session::SessionChange::Resolved {
                    id: key.clone(),
                    resolution: json!({"submitted":true,"accepted":null}),
                },
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_responses_and_request_specific_choices() {
        let request =
            |method: &str, params| Request { at: jiff::Timestamp::now(), id: json!(0), method: method.into(), params };
        let command =
            request("item/commandExecution/requestApproval", json!({"availableDecisions":["accept","cancel"]}));
        assert!(command.validate(&json!({"decision":"accept"})).is_ok());
        assert!(command.validate(&json!({"decision":"acceptForSession"})).is_err());
        assert!(command.validate(&json!({"decision":"approved"})).is_err());
        let legacy = request("execCommandApproval", json!({}));
        assert!(legacy.validate(&json!({"decision":"approved"})).is_ok());
        assert!(legacy.validate(&json!({"decision":"accept"})).is_err());
        let permissions = request("item/permissions/requestApproval", json!({}));
        assert!(permissions.validate(&json!({"permissions":{},"scope":"turn"})).is_ok());
        assert!(permissions.validate(&json!({"permissions":{"network":{"enabled":"yes"}}})).is_err());
        let form = request(
            "mcpServer/elicitation/request",
            json!({"mode":"form","requestedSchema":{"type":"object","required":["count"],"properties":{"count":{"type":"integer","minimum":1}}}}),
        );
        assert!(form.validate(&json!({"action":"accept","content":{"count":1},"_meta":null})).is_ok());
        assert!(form.validate(&json!({"action":"accept","content":{"count":0},"_meta":null})).is_err());
    }
}
