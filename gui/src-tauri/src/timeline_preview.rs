//! A live item can grow beyond a page before its durable object is available.
//! Keep a visibly incomplete preview until materialization supplies that object.

use fastctx_types::session::{Content, Happened, SessionEvent, What};

pub(crate) const LIVE_EVENT_BYTES: usize = 256 << 10;
const TEXT_BYTES: usize = 32 << 10;
const CONTINUES: &str = "\n… [The remaining content is being saved to the timeline.]";

fn clipped(text: &mut String) {
    if text.len() <= TEXT_BYTES {
        return;
    }
    let mut end = TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(CONTINUES);
}

pub(crate) fn limit(event: &mut SessionEvent) -> bool {
    if matches!(
        event.happened,
        Happened::Changed {
            change: fastctx_types::session::SessionChange::Request { .. }
                | fastctx_types::session::SessionChange::Resolved { .. },
            ..
        }
    ) {
        return false;
    }
    if serde_json::to_vec(event).map_or(0, |bytes| bytes.len()) <= LIVE_EVENT_BYTES {
        return false;
    }
    match &mut event.happened {
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => {
            item.detail = None;
            match &mut item.what {
                What::Message { text } | What::Thinking { text, .. } | What::Plan { text } => clipped(text),
                What::Tool(tool) => {
                    tool.input = serde_json::Value::Null;
                    tool.output.truncate(3);
                    for part in &mut tool.output {
                        if let Content::Text { text } = part {
                            clipped(text);
                        }
                    }
                    if serde_json::to_vec(tool).map_or(0, |bytes| bytes.len()) > LIVE_EVENT_BYTES / 2 {
                        let label = format!("{} — content is being saved to the timeline", tool.name);
                        item.what = What::Other { host_kind: label };
                    }
                }
                _ => {}
            }
        }
        Happened::Heard { text, message, notice, .. } => {
            clipped(text);
            if let Some(message) = message {
                message.attachments.clear();
                message.images.clear();
                message.content.clear();
                message.hook_fragments.clear();
                message.presentation = serde_json::Value::Null;
                message.detail = serde_json::Value::Null;
            }
            *notice = None;
        }
        Happened::Grew { text, .. } | Happened::RealtimeGrew { text, .. } => clipped(text),
        _ => {}
    }

    true
}
