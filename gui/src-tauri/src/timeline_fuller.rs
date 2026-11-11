//! A deferred preview must not shorten text that this reader already received.

use fastctx_types::session::{Content, Happened, Item, SessionEvent, What};

fn item(event: &SessionEvent) -> Option<&Item> {
    match &event.happened {
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => Some(item),
        _ => None,
    }
}
fn longer(preview: &mut String, received: &str) -> bool {
    let prefix = preview.strip_suffix('…').unwrap_or(preview);
    if received.len() > preview.len() && received.starts_with(prefix) {
        *preview = received.to_owned();
        true
    } else {
        false
    }
}

pub(crate) fn preserve(preview: &mut SessionEvent, received: &SessionEvent) -> bool {
    let Some(received) = item(received) else { return false };
    let (Happened::Finished { item: preview, .. }
    | Happened::Began { item: preview, .. }
    | Happened::Updated { item: preview, .. }) = &mut preview.happened
    else {
        return false;
    };
    match (&mut preview.what, &received.what) {
        (What::Message { text }, What::Message { text: old })
        | (What::Thinking { text, .. }, What::Thinking { text: old, .. })
        | (What::Plan { text }, What::Plan { text: old }) => longer(text, old),
        (What::Tool(tool), What::Tool(old)) => {
            let prefix = tool.output.len() <= old.output.len()
                && tool.output.iter().zip(&old.output).all(|(part, old)| match (part, old) {
                    (Content::Text { text }, Content::Text { text: old }) => {
                        old.starts_with(text.strip_suffix('…').unwrap_or(text))
                    }
                    _ => part == old,
                });
            let mut changed = false;
            for (part, old) in tool.output.iter_mut().zip(&old.output) {
                if let (Content::Text { text }, Content::Text { text: old }) = (part, old) {
                    changed |= longer(text, old);
                }
            }
            if prefix && tool.output.len() < old.output.len() {
                tool.output.extend_from_slice(&old.output[tool.output.len()..]);
                changed = true;
            }
            changed
        }
        _ => false,
    }
}
