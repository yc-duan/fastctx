//! Pure preparation of a bounded, parsed display body. The original source must
//! already be retained. Image decoding/selector capture happens before slimming;
//! these functions neither read image bytes nor reconstruct native history.

use fastctx_types::{
    run::{ExitStatus, RunState},
    run_facts::RunFacts,
    session::{Content, Does, Happened, Item, SessionChange, SessionEvent, ToolStatus, UserMessage, What},
    sync::{cursor::ByteCount, identity::Digest},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextStamp {
    pub client: String,
    pub bytes: ByteCount,
    pub digest: Digest,
}

/// Hides only a byte-exact injected first block tied to this client receipt.
/// The original blocks remain intact until the separate display-slimming step.
pub fn restore_context(event: &mut SessionEvent, stamp: &ContextStamp) -> bool {
    let Happened::Heard { text, message: Some(message), .. } = &mut event.happened else { return false };
    if message.client_id.as_deref() != Some(stamp.client.as_str()) {
        return false;
    }
    let Some(first) = message.content.first().and_then(|block| block["text"].as_str()) else { return false };
    if first.len() as u64 != stamp.bytes.0 || Digest::of(first.as_bytes()) != stamp.digest {
        return false;
    }
    *text = message
        .content
        .iter()
        .skip(1)
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    true
}

/// Used only after a native key or verified alias proves these are one input.
/// World speaker/delivery/notice/text remain outside this native metadata merge.
pub fn merge_user_metadata(world: &mut UserMessage, host: &mut UserMessage) {
    world.images = std::mem::take(&mut host.images);
    world.content = std::mem::take(&mut host.content);
    world.detail = host.detail.take();
    world.presentation = host.presentation.take();
    world.hook_fragments = std::mem::take(&mut host.hook_fragments);
    world.client_id = host.client_id.take();
}

/// Returns false for presentation-only runtime requests and whole-turn diffs.
/// Decoded image references and native selectors are supplied by the adapter;
/// invalid or unresolved image content is never silently rewritten here.
pub fn slim(event: &mut SessionEvent) -> bool {
    match &mut event.happened {
        Happened::Heard { message: Some(message), .. } => {
            message.content.clear();
            message.hook_fragments.clear();
            message.presentation = Value::Null;
            message.detail = Value::Null;
        }
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            slim_item(item)
        }
        Happened::Steps { steps, .. } => {
            for step in steps {
                step.detail = None;
            }
        }
        Happened::Changed {
            change: SessionChange::Diff { .. } | SessionChange::Request { .. } | SessionChange::Resolved { .. },
            ..
        } => return false,
        _ => {}
    }
    true
}
fn slim_item(item: &mut Item) {
    item.detail = match (&item.what, item.detail.take()) {
        (What::Message { .. }, Some(detail)) => detail.get("questions").filter(|q| q.is_array()).map(|q| {
            let mut value = serde_json::Map::new();
            value.insert("questions".into(), q.clone());
            Value::Object(value)
        }),
        _ => None,
    };
    let What::Tool(tool) = &mut item.what else { return };
    if matches!(tool.does, Does::Command { .. } | Does::Edit { .. }) {
        tool.input = Value::Null;
    }
    if tool.run.is_some() && matches!(tool.does, Does::Command { .. }) && tool.status != ToolStatus::Running {
        tool.output.retain(|value| !matches!(value, Content::Text { .. }));
    }
}

/// The caller has already resolved this call and filtered unavailable job facts.
/// This preserves legacy rendering semantics without loading a job or output.
pub fn attach_run(event: &mut SessionEvent, call: &str, facts: RunFacts) -> bool {
    let item = match &mut event.happened {
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => item,
        _ => return false,
    };
    if item.id != call {
        return false;
    }
    let What::Tool(tool) = &mut item.what else { return false };
    if tool.name != "run" || !tool.server.as_deref().is_some_and(|s| s == "fastctx" || s.ends_with("_fastctx")) {
        return false;
    }
    tool.machine = Some(facts.machine.clone());
    if let Does::Command { exit, job, .. } = &mut tool.does {
        *exit = match facts.state {
            RunState::Ended { status: ExitStatus::Code(code) }
            | RunState::Exited { status: ExitStatus::Code(code), .. } => Some(code),
            _ => None,
        };
        *job = if matches!(facts.state, RunState::Running | RunState::Exited { remaining: 1.., .. }) {
            facts.job.as_ref().map(ToString::to_string)
        } else {
            None
        };
    }
    tool.run = Some(Box::new(facts));
    true
}
