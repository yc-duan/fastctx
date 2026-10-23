//! What an agent asked the user, kept in its `now` for whoever calls the
//! user about it (`world-gui.md`, 通知): a request its turn waits on, from
//! either host, or a question Codex asked without waiting. A request is
//! asked until it is settled or its turn ends; a question asked without
//! waiting, until the user answers it or the agent asks another.

use std::sync::Arc;

use fastctx_types::ResidentId;
use fastctx_types::residents::{ASKED_CHARS, ASYNC_REPLY_TAG, Asked, AskedFor};
use fastctx_types::session::{Item, What};
use jiff::Timestamp;
use serde_json::Value;

use crate::Node;
use crate::turns::opening;

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
        "item/tool/requestUserInput" | "AskUserQuestion" => {
            let (said, options) = questions(&params["questions"], "question");
            (AskedFor::Answer, said, options)
        }
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

/// The questions Codex asked without waiting (`request_user_input_async`):
/// a message of the agent's that carries them.
pub fn of_item(item: &Item, at: Timestamp) -> Option<Asked> {
    if !matches!(item.what, What::Message { .. }) {
        return None;
    }
    let detail = item.detail.as_ref()?;
    if detail["delivery"].as_str() != Some("async") || detail["questions"].as_array().is_none_or(Vec::is_empty) {
        return None;
    }
    let (text, options) = questions(&detail["questions"], "title");
    Some(Asked { id: item.id.clone(), at, waits: false, what: AskedFor::Answer, text, options })
}

/// Each question on a line, at most `ASKED_CHARS` of it; the options when
/// there is a single question to choose for.
fn questions(questions: &Value, field: &str) -> (String, Vec<String>) {
    let list = questions.as_array().map(Vec::as_slice).unwrap_or_default();
    let text = list
        .iter()
        .map(|question| opening(question[field].as_str().unwrap_or_default(), ASKED_CHARS))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let options = match list {
        [question] => question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| option.as_str().or_else(|| option["label"].as_str()))
            .map(str::to_string)
            .collect(),
        _ => vec![],
    };
    (text, options)
}

/// Whether the user's words answer a question asked without waiting: they
/// carry Codex's reply envelope naming its item.
pub fn answers(words: &str, asked: &Asked) -> bool {
    if asked.waits {
        return false;
    }
    let close = ASYNC_REPLY_TAG.replacen('<', "</", 1);
    let Some(body) = words.trim().strip_prefix(ASYNC_REPLY_TAG).and_then(|body| body.strip_suffix(&close)) else {
        return false;
    };
    let Ok(answers) = serde_json::from_str::<Vec<Value>>(body) else { return false };
    answers.iter().any(|answer| {
        let Some(identity) = answer["questionItemId"].as_str() else { return false };
        let Ok(identity) = serde_json::from_str::<Vec<Value>>(identity) else { return false };
        matches!(identity.as_slice(), [kind, item, index]
            if kind.as_str() == Some("request_user_input_async")
                && item.as_str() == Some(asked.id.as_str()) && index.as_u64().is_some())
            && answer["answer"].as_str().is_some_and(|answer| !answer.trim().is_empty())
    })
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
