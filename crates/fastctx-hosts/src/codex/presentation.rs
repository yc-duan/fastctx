//! Content-dependent projection rules shared by live and historical items.

use serde_json::{Value, json};

pub fn hidden_tool(item: &Value) -> bool {
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
    let namespace = item.get("namespace").or_else(|| item.get("server")).and_then(Value::as_str);
    (matches!(namespace, None | Some("codex_app")) && matches!(tool, "update_running_summary" | "update_up_next"))
        || tool == "load_workspace_dependencies"
        || (tool == "automation_update"
            && !(item["status"] == "completed" && item["success"] == true && automation_arguments(&item["arguments"])))
}

fn automation_arguments(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    let text = |key: &str| value.get(key).and_then(Value::as_str).filter(|v| !v.trim().is_empty());
    let Some(mode) = text("mode") else { return false };
    if matches!(mode, "view" | "delete") {
        return text("id").is_some() && object.keys().all(|k| matches!(k.as_str(), "mode" | "id"));
    }
    if !matches!(mode, "create" | "suggested_create" | "update" | "suggested_update") {
        return false;
    }
    if matches!(mode, "update" | "suggested_update") && text("id").is_none() {
        return false;
    }
    if ["name", "prompt", "rrule"].iter().any(|k| text(k).is_none())
        || !matches!(text("status"), Some("ACTIVE" | "PAUSED"))
    {
        return false;
    }
    if value.get("notificationPolicy").is_some_and(|v| !v.is_null() && v != "failed_runs_only") {
        return false;
    }
    let common = ["mode", "kind", "name", "prompt", "rrule", "status", "notificationPolicy"];
    let known = |key: &str| common.contains(&key) || (key == "id" && matches!(mode, "update" | "suggested_update"));
    match text("kind") {
        Some("cron") => {
            value.get("projectId").is_some_and(|v| v.is_null() || text("projectId").is_some())
                && text("model").is_some()
                && matches!(
                    text("reasoningEffort"),
                    Some("none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra")
                )
                && matches!(text("executionEnvironment"), Some("local" | "worktree"))
                && value.get("destination").is_none_or(|v| v == "local" || v == "worktree")
                && value
                    .get("localEnvironmentConfigPath")
                    .is_none_or(|v| v.is_null() || text("localEnvironmentConfigPath").is_some())
                && object.keys().all(|k| {
                    known(k.as_str())
                        || [
                            "projectId",
                            "model",
                            "reasoningEffort",
                            "executionEnvironment",
                            "destination",
                            "localEnvironmentConfigPath",
                        ]
                        .contains(&k.as_str())
                })
        }
        Some("heartbeat") => {
            (text("targetThreadId").is_some() || text("destination") == Some("thread"))
                && value.get("destination").is_none_or(|v| v == "local" || v == "thread")
                && value.get("targetThreadId").is_none_or(|_| text("targetThreadId").is_some())
                && object.keys().all(|k| known(k.as_str()) || ["destination", "targetThreadId"].contains(&k.as_str()))
        }
        _ => false,
    }
}

pub fn function_message(item: &Value) -> Option<Value> {
    let output = item.get("output")?.as_str()?;
    let name = item.get("name")?.as_str()?;
    let namespace = item.get("namespace").and_then(Value::as_str);
    let delegation = ((namespace == Some("codex_app")
        && matches!(name, "create_thread" | "send_message_to_thread" | "handoff_thread"))
        || (namespace == Some("cloud_threads") && name == "send_message"))
        && wrapped(output, "codex_delegation")
            .is_some_and(|body| tag(body, "source_thread_id").is_some() && tag(body, "input").is_some());
    let automation = namespace == Some("codex_app")
        && name == "automation_update"
        && ((output.starts_with("Automation: ") && output.contains("\nAutomation ID: "))
            || heartbeat(output).is_some());
    (delegation || automation).then(|| json!({"type":"userMessage", "id":item["id"], "content":[{"type":"text", "text":output, "text_elements":[]}], "toolOutput":true}))
}

fn wrapped<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.trim().strip_prefix(&format!("<{name}>"))?.strip_suffix(&format!("</{name}>"))
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find(&format!("<{name}>"))? + name.len() + 2;
    let end = lower[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

fn decode(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn heartbeat(text: &str) -> Option<Value> {
    let body = wrapped(text, "heartbeat")?;
    Some(
        json!({"current_time_iso":tag(body,"current_time_iso")?, "instructions":tag(body,"instructions")?, "automation_id":tag(body,"automation_id")}),
    )
}

pub fn user_message(item: &Value) -> Option<(String, Value)> {
    let content = item.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    if content.iter().any(|part| {
        part.get("text_elements").and_then(Value::as_array).is_some_and(|spans| {
            spans.iter().any(|span| {
                matches!(span.get("placeholder").and_then(Value::as_str), Some("aeon-continuation" | "aeon-onboarding"))
            })
        })
    }) {
        return None;
    }
    let texts: Vec<&str> = content.iter().filter(|v| v["type"] == "text").filter_map(|v| v["text"].as_str()).collect();
    let text = texts.join("\n");
    let mut presentation = json!({"tool_output": item.get("toolOutput").and_then(Value::as_bool).unwrap_or(false)});
    if let Some(heartbeat) = heartbeat(&text) {
        presentation["heartbeat"] = heartbeat.clone();
        return Some((heartbeat["instructions"].as_str().unwrap().into(), presentation));
    }
    if let Some(body) = wrapped(&text, "codex_delegation")
        && let (Some(source), Some(input)) = (tag(body, "source_thread_id"), tag(body, "input"))
    {
        presentation["delegation"] = json!({"source_thread_id":decode(source)});
        return Some((decode(input), presentation));
    }
    if let Some(body) = wrapped(&text, "codex_automation_message")
        && let Some(automation) = automation_message(body)
    {
        let visible = automation["body"].as_str().unwrap().to_string();
        presentation["automation"] = automation;
        return Some((visible, presentation));
    }
    let external_client = item
        .get("clientId")
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with("chatgpt-messaging:") || s.starts_with("aeon-imessage:"));
    if !external_client
        && content.len() == texts.len()
        && matches!(texts.len(), 1 | 2)
        && let Some(external) = external_event(texts[texts.len() - 1], (texts.len() == 2).then(|| texts[0]))
    {
        let orbit = external["orbit"].as_bool().unwrap_or(false);
        if texts.len() == 1 || orbit {
            let visible = if orbit && texts.len() == 2 {
                texts[0].to_string()
            } else {
                external.pointer("/payload/text").and_then(Value::as_str).unwrap_or_default().to_string()
            };
            presentation["external_event"] = external;
            return Some((visible, presentation));
        }
    }
    Some((text, presentation))
}

fn automation_message(mut body: &str) -> Option<Value> {
    let mut fields = serde_json::Map::new();
    for (name, required) in
        [("automation_id", true), ("run_id", false), ("title", true), ("body", true), ("hidden_context", false)]
    {
        if let Some(rest) = body.strip_prefix(&format!("<{name}>")) {
            let (text, rest) = rest.split_once(&format!("</{name}>"))?;
            if text.contains('<') {
                return None;
            }
            if name != "hidden_context" {
                fields.insert(name.into(), json!(decode(text)));
            }
            body = rest;
        } else if required {
            return None;
        }
    }
    body.is_empty().then_some(Value::Object(fields))
}

fn external_event(text: &str, preview: Option<&str>) -> Option<Value> {
    let text = text.trim();
    if !text.starts_with("<external_event") {
        return None;
    }
    let document = roxmltree::Document::parse(text).ok()?;
    let root = document.root_element();
    if root.tag_name().name() != "external_event" || root.children().any(|n| n.is_element()) {
        return None;
    }
    if root.attribute("source_kind") != Some("mcp") || root.attribute("source_id") != Some("codex_apps") {
        return None;
    }
    let attr = |name| root.attribute(name).filter(|v| !v.trim().is_empty());
    let subscription = attr("subscription_id")?;
    let name = attr("event_name")?;
    let source = attr("connector_slug").or_else(|| {
        name.split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
            .then(|| name.split('.').next().unwrap())
    });
    let body: String = root.children().filter(|n| n.is_text()).filter_map(|n| n.text()).collect();
    if let Some(status) = root.attribute("status") {
        return (status == "ended" && body.trim() == "The event subscription ended.").then(|| json!({"id":format!("external-event:{subscription}:ended"),"subscription_id":subscription,"event_name":name,"status":status,"source":source}));
    }
    let event_id = attr("event_id")?;
    let timestamp = attr("timestamp")?;
    let payload: Value = serde_json::from_str(&body).ok()?;
    if !payload.is_object() {
        return None;
    }
    let attachments = payload["attachments"].as_array().filter(|a| {
        a.iter().all(|attachment| {
            attachment["file_name"].as_str().is_some_and(|s| !s.is_empty())
                && matches!(
                    attachment["status"].as_str(),
                    Some("available" | "unavailable" | "rejected" | "timed_out" | "omitted")
                )
                && (attachment["file_uri"].is_null()
                    || attachment["file_uri"]["file_id"].as_str().is_some_and(|s| !s.is_empty()))
        })
    });
    let orbit = source.is_some_and(|source| {
        name == format!("{source}.orbit.message") && payload["surface"] == source && payload["direction"] == "incoming"
    }) && (preview.is_some_and(|s| !s.is_empty())
        || payload["text"].as_str().is_some_and(|s| !s.is_empty())
        || attachments.is_some_and(|a| !a.is_empty()));
    Some(
        json!({"id":format!("external-event:{subscription}:{event_id}"),"subscription_id":subscription,"event_name":name,"timestamp":timestamp,"source":source,"payload":payload,"orbit":orbit}),
    )
}
