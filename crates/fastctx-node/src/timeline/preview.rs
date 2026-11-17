//! What a page shows in place of an event too large for it: the same event
//! with what is long in it cut short. It is still an event, and the one it
//! stands for by everything that tells events apart (its time, its kind, its
//! turn, its item, its words' name), so a viewer places and draws it like
//! any other and fetches the whole only to show the rest.

use super::identity;
use fastctx_types::session::{Content, Happened, SessionEvent, What};
use serde_json::Value;

/// The largest preview, as JSON.
const BYTES: usize = 32 * 1024;
/// How many characters a text keeps and how many entries a list keeps, from
/// the most generous to the least: the first that fits is the preview.
const KEPT: [(usize, usize); 4] = [(8_192, 256), (2_048, 64), (512, 16), (256, 4)];
/// The entries a map keeps. An event's own structures have far fewer fields;
/// only what a host or a tool put in freely has more.
const FIELDS: usize = 64;
/// What ends a text that was cut.
const CUT: char = '…';

/// The event cut down to a preview; none when no cut makes it small enough
/// and still the same event.
pub(super) fn preview(event: &SessionEvent) -> Option<Value> {
    let mut shown = event.clone();
    // What no interface draws goes first. Whether one draws the event at all
    // stays the viewer's to say, as for an event that fits a page.
    fastctx_net::history::slim(&mut shown, &fastctx_net::history::Pictures::Inline);
    pictures(&mut shown);
    let shown = serde_json::to_value(&shown).ok()?;
    KEPT.iter().find_map(|&(text, entries)| {
        let mut cut = shown.clone();
        shorten(&mut cut, text, entries);
        if serde_json::to_vec(&cut).ok()?.len() > BYTES {
            return None;
        }
        let read: SessionEvent = serde_json::from_value(cut.clone()).ok()?;
        same(event, &read).then_some(cut)
    })
}

/// Whether `cut` is the event `whole` is, to one that tells events apart.
fn same(whole: &SessionEvent, cut: &SessionEvent) -> bool {
    whole.at == cut.at
        && std::mem::discriminant(&whole.happened) == std::mem::discriminant(&cut.happened)
        && identity::name(whole) == identity::name(cut)
        && turn(whole) == turn(cut)
}

fn turn(event: &SessionEvent) -> Option<&str> {
    match &event.happened {
        Happened::Began { turn, .. }
        | Happened::Updated { turn, .. }
        | Happened::Finished { turn, .. }
        | Happened::Grew { turn, .. }
        | Happened::TurnStarted { turn }
        | Happened::TurnEnded { turn, .. } => Some(turn),
        _ => None,
    }
}

/// Takes the images a tool gave back as bytes out of the event: cut short
/// they would be no image.
fn pictures(event: &mut SessionEvent) {
    let (Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. }) =
        &mut event.happened
    else {
        return;
    };
    if let What::Tool(tool) = &mut item.what {
        tool.output.retain(|content| !matches!(content, Content::Image { .. }));
    }
}

fn shorten(value: &mut Value, text: usize, entries: usize) {
    match value {
        Value::String(string) => {
            if let Some((end, _)) = string.char_indices().nth(text) {
                string.truncate(end);
                string.push(CUT);
            }
        }
        Value::Array(list) => {
            list.truncate(entries);
            for entry in list {
                shorten(entry, text, entries);
            }
        }
        Value::Object(map) => {
            if map.len() > FIELDS {
                let dropped: Vec<String> = map.keys().skip(FIELDS).cloned().collect();
                for key in dropped {
                    map.remove(&key);
                }
            }
            for entry in map.values_mut() {
                shorten(entry, text, entries);
            }
        }
        _ => {}
    }
}
