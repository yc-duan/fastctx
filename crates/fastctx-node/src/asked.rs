//! What an agent asked the user, kept in its `now` for whoever calls the
//! user about it (`world-gui.md`, 通知): a request its turn waits on, from
//! either host, asked until it is settled or its turn ends. A question Codex
//! asks without stopping is words to the user (`asked_without_stopping`),
//! said as `message_user`'s are.

use std::sync::Arc;

use fastctx_types::ResidentId;
use fastctx_types::residents::{Asked, AskedFor};
use fastctx_types::session::{Item, What};
use jiff::Timestamp;
use serde_json::Value;

use crate::Node;

/// Requests the node answers itself, never a person.
const MACHINE_ONLY: [&str; 4] =
    ["account/chatgptAuthTokens/refresh", "attestation/generate", "item/tool/call", "currentTime/read"];

/// What a request from its host asks the user, read as the interface reads
/// it (`gui/src/conversation/asks.ts`); none for one no person answers.
pub fn of_request(id: &Value, request: &str, params: &Value, at: Timestamp) -> Option<Asked> {
    if MACHINE_ONLY.contains(&request) {
        return None;
    }
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let (what, said, options) = match request {
        "item/commandExecution/requestApproval" => (AskedFor::Command, text(&params["command"]), vec![]),
        "execCommandApproval" => {
            let argv: Vec<_> = params["command"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            (AskedFor::Command, argv.join(" "), vec![])
        }
        "item/fileChange/requestApproval" => (AskedFor::Change, text(&params["reason"]), vec![]),
        "applyPatchApproval" => {
            let paths: Vec<_> =
                params["fileChanges"].as_object().into_iter().flat_map(|files| files.keys()).cloned().collect();
            (AskedFor::Change, paths.join("\n"), vec![])
        }
        "classic.PermissionRequest" | "tool.check" => {
            let (tool, input) = match request {
                "tool.check" => (&params["tool"], &params["input"]),
                _ => (&params["tool_name"], &params["tool_input"]),
            };
            match tool.as_str().unwrap_or_default() {
                "Bash" => (AskedFor::Command, text(&input["command"]), vec![]),
                "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                    let path = input["file_path"].as_str().or_else(|| input["notebook_path"].as_str());
                    (AskedFor::Change, path.unwrap_or_default().to_string(), vec![])
                }
                "ExitPlanMode" => (AskedFor::Plan, text(&input["plan"]), vec![]),
                tool => (AskedFor::Tool, tool.to_string(), vec![]),
            }
        }
        "Elicitation" | "mcpServer/elicitation/request" => (AskedFor::Form, text(&params["message"]), vec![]),
        "item/permissions/requestApproval" => (AskedFor::Other, text(&params["reason"]), vec![]),
        _ => (AskedFor::Other, String::new(), vec![]),
    };
    let id = id.as_str().map(str::to_string).unwrap_or_else(|| id.to_string());
    Some(Asked { id, at, waits: true, what, text: said, options })
}

/// What Codex asked the user without stopping (`request_user_input_async`,
/// which a World thread cannot be without): an agent's message carrying its
/// questions, as the words it says, its own and then each question with the
/// answers it offers.
pub fn asked_without_stopping(item: &Item) -> Option<String> {
    let What::Message { text } = &item.what else { return None };
    let detail = item.detail.as_ref()?;
    if detail["delivery"].as_str() != Some("async") {
        return None;
    }
    let questions = detail["questions"].as_array().filter(|list| !list.is_empty())?;
    let asked = questions.iter().map(|question| {
        let title = format!("**{}**", question["title"].as_str().unwrap_or_default().trim());
        let options = question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| option.as_str().or_else(|| option["label"].as_str()))
            .map(|option| format!("- {option}"));
        std::iter::once(title).chain(options).collect::<Vec<_>>().join("\n")
    });
    let said = std::iter::once(text.trim().to_string()).filter(|own| !own.is_empty()).chain(asked);
    Some(said.collect::<Vec<_>>().join("\n\n"))
}

/// Keeps `asked` as what the resident asks now, unless it is already.
pub fn ask(node: &Arc<Node>, id: &ResidentId, asked: Asked) -> std::io::Result<()> {
    if node.residents.now(id).and_then(|now| now.asked).is_some_and(|now| now.id == asked.id) {
        return Ok(());
    }
    node.residents.change_now(&node.world, id, |now| now.asked = Some(asked))
}

/// It is no longer asked, when `settled` says so of what is.
pub fn settle(node: &Node, id: &ResidentId, settled: impl FnOnce(&Asked) -> bool) -> std::io::Result<()> {
    if !node.residents.now(id).and_then(|now| now.asked).is_some_and(|asked| settled(&asked)) {
        return Ok(());
    }
    node.residents.change_now(&node.world, id, |now| now.asked = None)
}
