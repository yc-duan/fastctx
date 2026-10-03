//! The course of a turn in a session the World holds (`specs/world-colleagues.md`, "一轮怎样
//! 结束"): whose words went into it and so are owed its end, what its host reported about the
//! session while it ran, and how it ended.
//!
//! A turn ends one of three ways: its host reports the end (completed, failed or interrupted);
//! the host's process stops under it; or the node stops while it runs and, starting again,
//! finds it unfinished. All three end in [`Town::turn_ended`], so the resident's records,
//! everyone watching it and everyone owed its end hear one account. Who is owed it is kept in
//! the resident's `now` beside the turn, so a node that restarts still knows whom to tell.

use super::{
    Birth, Delivery, Live, MessageLine, Records, State, Town, delivery_id, format_duration,
};
use crate::town::records::Trouble;
use crate::town::remote::{Letter, Peer};
use crate::town::talk::TalkEvent;
use std::collections::BTreeSet;

/// How a turn ended, and who is owed its last message.
pub(super) struct TurnEnd {
    /// The turn, as its host named it; unknown for a turn the node found unfinished in records
    /// written before turns were recorded.
    pub(super) turn: Option<String>,
    /// `completed`, `failed`, `interrupted`, or whatever else the host calls it.
    pub(super) status: String,
    /// Why it did not complete, when the host or the World knows.
    pub(super) reason: Option<String>,
    pub(super) duration_ms: Option<u64>,
    pub(super) last_message: Option<String>,
    pub(super) readers: Vec<Peer>,
    /// The turn did not end on its own: the host's process or the node stopped under it.
    pub(super) outage: bool,
}

/// Something a host reported about a session rather than its work.
pub(super) struct Report {
    /// What it was, in one sentence, in the host's own words where it gives them.
    pub(super) text: String,
    /// Set when everyone owed the turn's end must hear it at once: the session cannot do its
    /// work, or its model's provider may stop the turn. Each kind is told once per turn.
    pub(super) tell: Option<&'static str>,
}

impl Live {
    pub(super) fn new(
        records: Records,
        about: super::About,
        route: super::Route,
        inbox_address: Option<String>,
        sessions: u32,
    ) -> Self {
        Self {
            records,
            about,
            route,
            inbox_address,
            sessions,
            readers: Default::default(),
            turn: None,
            told: BTreeSet::new(),
        }
    }

    /// Everyone owed the end of a turn of this session, whichever turn their words went into.
    fn owed(&self) -> Vec<Peer> {
        self.readers
            .values()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The session runs `turn`; what was told about an earlier one is not told about it.
    fn enter(&mut self, turn: &str) {
        if self.turn.as_deref() != Some(turn) {
            self.turn = Some(turn.to_string());
            self.told.clear();
        }
    }
}

impl Town {
    /// A turn started in a resident's session, as its host reported; returns its records.
    pub(super) fn turn_started(&self, id: &str, turn: &str) -> Option<Records> {
        let mut state = self.state.lock();
        let live = state.residents.get_mut(id)?;
        live.enter(turn);
        self.mirror(live);
        Some(live.records.clone())
    }

    /// Words of `peers` went into `turn` of a resident's session: each is owed its end.
    /// Returns the resident's records.
    pub(super) fn owe(
        &self,
        id: &str,
        turn: &str,
        peers: impl IntoIterator<Item = Peer>,
    ) -> Option<Records> {
        let mut state = self.state.lock();
        let live = state.residents.get_mut(id)?;
        live.enter(turn);
        live.readers
            .entry(turn.to_string())
            .or_default()
            .extend(peers);
        self.mirror(live);
        Some(live.records.clone())
    }

    /// The residents owed a turn's end, taken once it ended; the turn is over.
    pub(super) async fn take_readers(&self, id: &str, turn: &str) -> Vec<Peer> {
        let _turns = self.turns.lock().await;
        let mut state = self.state.lock();
        let Some(live) = state.residents.get_mut(id) else {
            return Vec::new();
        };
        if live.turn.as_deref() == Some(turn) {
            live.turn = None;
            live.told.clear();
        }
        let readers = live
            .readers
            .remove(turn)
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.mirror(live);
        readers
    }

    /// Keeps a resident's `now` telling the turn it runs and who is owed its end.
    fn mirror(&self, live: &Live) {
        let owed = live.owed();
        let changed = live.records.update_now(|now| {
            now.turn = live.turn.clone();
            now.owed = owed;
        });
        if let Err(error) = changed {
            (self.log)(format!(
                "cannot record the turn of {}: {error}",
                live.about.id
            ));
        }
    }

    /// A turn of a resident's session ended: everyone watching sees how, its records say so,
    /// and its last message goes, verbatim, to everyone owed it.
    pub(super) async fn turn_ended(&self, id: &str, end: TurnEnd) {
        let TurnEnd {
            turn,
            status,
            reason,
            duration_ms,
            last_message,
            readers,
            outage,
        } = end;
        if let Some(turn) = &turn {
            self.publish(
                id,
                TalkEvent::TurnEnded {
                    turn: turn.clone(),
                    status: status.clone(),
                    reason: reason.clone(),
                },
            );
        }
        let (records, requester_here, newer) = {
            let state = self.state.lock();
            let Some(live) = state.residents.get(id) else {
                return;
            };
            (
                live.records.clone(),
                self.requester_here(&state, live),
                live.turn.is_some(),
            )
        };
        let ended = crate::world::now_rfc3339();
        let names: Vec<String> = readers
            .iter()
            .map(|reader| self.peer_name(reader))
            .collect();
        let changed = records.update_now(|now| {
            // Words that arrived meanwhile may already run the next turn; that turn's state
            // stands. A turn something stopped under stays interrupted until the next words.
            if !newer {
                now.state = if outage {
                    "interrupted"
                } else if requester_here {
                    "idle"
                } else {
                    "asleep"
                }
                .to_string();
                now.since = ended.clone();
            }
            if !names.is_empty() {
                now.last_action = Some(format!("answered {}", names.join(", ")));
                now.last_action_at = Some(ended.clone());
            }
            now.trouble = match (status.as_str(), &reason) {
                ("completed", _) => None,
                (_, Some(reason)) => Some(Trouble {
                    at: ended.clone(),
                    text: format!("turn {status}: {reason}"),
                }),
                ("interrupted", None) => None,
                (_, None) => Some(Trouble {
                    at: ended.clone(),
                    text: format!("turn {status}"),
                }),
            };
        });
        if let Err(error) = changed {
            (self.log)(format!("cannot record the end of {id}'s turn: {error}"));
        }
        let written = records.append_message(&MessageLine {
            at: ended.clone(),
            kind: if status == "completed" {
                "answered".to_string()
            } else {
                status.clone()
            },
            peer: names.join(","),
            turn: turn.clone(),
            text: last_message.clone().or_else(|| reason.clone()),
        });
        if let Err(error) = written {
            (self.log)(format!("cannot record how {id}'s turn ended: {error}"));
        }
        let how = match (status.as_str(), reason.as_deref()) {
            ("completed", _) => "turn completed".to_string(),
            (other, Some(reason)) => format!("turn {other}: {reason}"),
            (other, None) => format!("turn {other}"),
        };
        let length = duration_ms
            .map(format_duration)
            .map(|length| format!(", {length}"))
            .unwrap_or_default();
        let text = format!(
            "=== colleague {id} on {} ({how}{length}) ===\n{}\n=== what it did: fastctx w {id} ===",
            self.config.machine,
            last_message
                .as_deref()
                .unwrap_or("(it ended the turn without a message)"),
        );
        self.tell(&records, turn.as_deref(), readers, (id, "answer"), &text)
            .await;
    }

    /// Its host reported something about a resident's session rather than its work. It goes
    /// into the resident's `now` and to everyone watching; a report that must be heard at once
    /// also goes to everyone owed the turn's end.
    pub(super) async fn health(&self, id: &str, report: Report) {
        let Some((records, readers, turn)) = self.trouble(id, &report) else {
            return;
        };
        if !readers.is_empty() {
            let text = format!(
                "=== colleague {id} on {} (in its turn) ===\n{}\n=== what it is doing: fastctx w {id} ===",
                self.config.machine, report.text
            );
            self.tell(
                &records,
                turn.as_deref(),
                readers,
                ("world", "notice"),
                &text,
            )
            .await;
        }
    }

    /// Records what a host reported about a resident's session in its `now` and shows it to
    /// everyone watching. Returns its records, who must hear it at once, and the turn it is in.
    pub(super) fn trouble(
        &self,
        id: &str,
        report: &Report,
    ) -> Option<(Records, Vec<Peer>, Option<String>)> {
        let (records, readers, turn) = {
            let mut state = self.state.lock();
            let live = state.residents.get_mut(id)?;
            let tell = report.tell.is_some_and(|kind| live.told.insert(kind));
            (
                live.records.clone(),
                if tell { live.owed() } else { Vec::new() },
                live.turn.clone(),
            )
        };
        let at = crate::world::now_rfc3339();
        let changed = records.update_now(|now| {
            now.trouble = Some(Trouble {
                at,
                text: report.text.clone(),
            })
        });
        if let Err(error) = changed {
            (self.log)(format!("cannot record what {id}'s host reported: {error}"));
        }
        self.publish(
            id,
            TalkEvent::Health {
                text: report.text.clone(),
            },
        );
        Some((records, readers, turn))
    }

    /// Gives a text about a resident's turn to each reader, on this machine or by mail, and
    /// records in the resident's messages how each went.
    async fn tell(
        &self,
        records: &Records,
        turn: Option<&str>,
        readers: Vec<Peer>,
        (from, kind): (&str, &str),
        text: &str,
    ) {
        for reader in readers {
            let delivery = match delivery_id() {
                Ok(id) => Delivery {
                    id,
                    from: from.to_string(),
                    kind: kind.to_string(),
                    text: text.to_string(),
                    as_tool_output: false,
                },
                Err(error) => {
                    (self.log)(error);
                    continue;
                }
            };
            // A reader that is not here now gets it when it comes back; one on another machine
            // is mailed, and its machine delivers or holds it.
            let outcome = if reader.machine == self.config.machine {
                self.deliver_or_hold(&reader.id, delivery)
                    .await
                    .map(|now| if now { "delivered" } else { "held" })
            } else {
                self.mail(
                    &reader.machine,
                    &Letter::Deliver {
                        to: reader.id.clone(),
                        delivery,
                    },
                )
                .map(|()| "sent")
            };
            let written = records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: outcome
                    .as_ref()
                    .map_or("undelivered", |done| done)
                    .to_string(),
                peer: self.peer_name(&reader),
                turn: turn.map(str::to_string),
                text: outcome.err(),
            });
            if let Err(error) = written {
                (self.log)(format!(
                    "cannot record what {} was told: {error}",
                    self.peer_name(&reader)
                ));
            }
        }
    }

    /// Whether the resident a session works for is here: a colleague's requester that has not
    /// left; a resident the user opened always is.
    fn requester_here(&self, state: &State, live: &Live) -> bool {
        match &live.about.birth {
            Birth::Colleague {
                requester,
                machine: None,
                ..
            } => self.present(state, requester),
            // A requester on another machine is here until its machine writes that it left.
            Birth::Colleague {
                requester,
                machine: Some(machine),
                ..
            } => !state.departed.contains(&Peer::new(machine, requester)),
            Birth::User => true,
        }
    }
}
