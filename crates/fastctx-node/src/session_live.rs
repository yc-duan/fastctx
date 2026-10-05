//! Running items for a watcher joining between their beginning and end.
//! These snapshots are never part of the durable session history.

use std::collections::HashSet;

use fastctx_types::session::{Content, Happened, Part, SessionChange, SessionEvent, What};

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
            Happened::Changed { change: SessionChange::Request { id, .. }, .. } => {
                self.items.retain(|snapshot| {
                    !matches!(&snapshot.happened,
                    Happened::Changed { change: SessionChange::Request { id: held, .. }, .. } if held == id)
                });
                self.items.push(event.clone());
            }
            Happened::Changed { change: SessionChange::Resolved { id, .. }, .. } => {
                self.items.retain(|snapshot| {
                    !matches!(&snapshot.happened,
                    Happened::Changed { change: SessionChange::Request { id: held, .. }, .. } if held == id)
                });
            }
            Happened::Began { turn, item } => {
                if self.items.iter().any(|event| {
                    matches!(&event.happened,
                    Happened::Began { turn: t, item: i } if t == turn && i.id == item.id)
                }) {
                    return false;
                }
                self.items.push(event.clone());
            }
            Happened::Updated { turn, item } => {
                let existing = self.items.iter_mut().find(|snapshot| {
                    matches!(&snapshot.happened, Happened::Began { turn: t, item: i } if t == turn && i.id == item.id)
                });
                if let Some(snapshot) = existing {
                    if let Happened::Began { item: current, .. } = &mut snapshot.happened {
                        let started_at = current.started_at;
                        *current = item.clone();
                        current.started_at = started_at.or(item.started_at);
                    }
                } else {
                    self.items.push(SessionEvent {
                        at: item.started_at.unwrap_or(event.at),
                        happened: Happened::Began { turn: turn.clone(), item: item.clone() },
                    });
                }
            }
            Happened::Grew { turn, item, part, text, .. } => {
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
            Happened::Realtime { item, complete } => {
                self.items.retain(|snapshot| !matches!(&snapshot.happened, Happened::Realtime { item: current, .. }
                    if current["realtimeSessionId"] == item["realtimeSessionId"] && (current["id"] == item["id"] || item["type"] == "realtimeSessionClosed")));
                if !complete {
                    self.items.push(event.clone());
                }
            }
            Happened::RealtimeGrew { item, text } => {
                for snapshot in &mut self.items {
                    if let Happened::Realtime { item: current, complete: false } = &mut snapshot.happened
                        && current["id"].as_str() == Some(item)
                    {
                        let mut whole = current["text"].as_str().unwrap_or_default().to_string();
                        whole.push_str(text);
                        current["text"] = whole.into();
                    }
                }
            }
            Happened::Changed { change: SessionChange::Voice { event, .. }, .. }
                if matches!(event.as_str(), "closed" | "error") =>
            {
                self.items.retain(|snapshot| !matches!(snapshot.happened, Happened::Realtime { .. }));
            }
            _ => {}
        }
        if self.items.is_empty() {
            self.deltas.clear();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::Item;
    use serde_json::json;

    #[test]
    fn late_watchers_keep_equal_chunks_replacements_and_voice_until_completion() {
        let at = jiff::Timestamp::from_millisecond(1_000).unwrap();
        let event = |happened| SessionEvent { at, happened };
        let item = |text: &str| Item {
            id: "same".into(),
            started_at: Some(at),
            detail: None,
            what: What::Thinking { text: text.into() },
        };
        let mut live = Live::default();
        live.observe(&event(Happened::Began { turn: "a".into(), item: item("") }));
        for revision in [1, 2] {
            live.observe(&event(Happened::Grew {
                turn: "a".into(),
                item: "same".into(),
                part: Part::Text,
                text: "x".into(),
                revision: Some(revision),
            }));
        }
        assert!(
            matches!(&live.snapshots().next().unwrap().happened, Happened::Began { item, .. } if item.what == What::Thinking { text: "xx".into() })
        );
        live.observe(&event(Happened::Updated { turn: "a".into(), item: item("replacement") }));
        live.observe(&event(Happened::Began { turn: "b".into(), item: item("independent") }));
        let voice = json!({"type":"transcriptSegment","id":"voice","realtimeSessionId":"session","role":"assistant","text":"prefix"});
        live.observe(&event(Happened::Realtime { item: voice.clone(), complete: false }));
        live.observe(&event(Happened::RealtimeGrew { item: "voice".into(), text: " suffix".into() }));
        assert_eq!(live.snapshots().count(), 3);
        assert!(
            live.snapshots()
                .any(|e| matches!(&e.happened, Happened::Realtime { item, .. } if item["text"] == "prefix suffix"))
        );
        live.observe(&event(Happened::Finished { turn: "a".into(), item: item("replacement") }));
        live.observe(&event(Happened::Realtime { item: voice, complete: true }));
        assert_eq!(live.snapshots().count(), 1);
        assert!(matches!(&live.snapshots().next().unwrap().happened, Happened::Began { turn, .. } if turn == "b"));
    }
}
