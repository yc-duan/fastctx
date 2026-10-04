//! Running items for a watcher joining between their beginning and end.
//! These snapshots are never part of the durable session history.

use std::collections::HashSet;

use fastctx_types::session::{Content, Happened, Part, SessionEvent, What};

#[derive(Default)]
pub struct Live {
    items: Vec<SessionEvent>,
    deltas: HashSet<String>,
}

impl Live {
    pub fn snapshots(&self) -> impl Iterator<Item = &SessionEvent> {
        self.items.iter()
    }

    pub fn observe(&mut self, event: &SessionEvent) -> bool {
        match &event.happened {
            Happened::Began { turn, item } => {
                if self.items.iter().any(|event| {
                    matches!(&event.happened,
                    Happened::Began { turn: t, item: i } if t == turn && i.id == item.id)
                }) {
                    return false;
                }
                self.items.push(event.clone());
            }
            Happened::Grew { turn, item, part, text } => {
                if !self.deltas.insert(serde_json::to_string(event).expect("session event serializes")) {
                    return false;
                }
                for snapshot in &mut self.items {
                    if let Happened::Began { turn: t, item: i } = &mut snapshot.happened
                        && t == turn
                        && i.id == *item
                    {
                        match (&mut i.what, part) {
                            (
                                What::Message { text: whole }
                                | What::Thinking { text: whole }
                                | What::Plan { text: whole },
                                Part::Text,
                            ) => whole.push_str(text),
                            (What::Tool(tool), Part::Output) => {
                                if let Some(Content::Text { text: whole }) = tool.output.last_mut() {
                                    whole.push_str(text);
                                } else {
                                    tool.output.push(Content::Text { text: text.clone() });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Happened::Finished { turn, item } => self.items.retain(|event| {
                !matches!(&event.happened,
                Happened::Began { turn: t, item: i } if t == turn && i.id == item.id)
            }),
            Happened::TurnEnded { turn, .. } => self.items.retain(|event| {
                !matches!(&event.happened,
                Happened::Began { turn: t, .. } if t == turn)
            }),
            _ => {}
        }
        if self.items.is_empty() {
            self.deltas.clear();
        }
        true
    }
}
