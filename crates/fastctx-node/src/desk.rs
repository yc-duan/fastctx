//! This machine's one Computer Use desk (`world-colleagues.md`, 一台机器只有
//! 一张桌子).
//!
//! The desktop app's helper takes one request at a time and refuses the
//! rest on the spot, without saying who holds it. The World adds what it can
//! see, and locks nothing: the first resident whose Computer Use call goes
//! through sits at the desk until its turn ends; a resident refused
//! meanwhile is told who sits there, or whose call is in flight, and that
//! it will hear when the desk is free. A refusal while no resident of the
//! World sits there or calls means a session outside the World holds it,
//! and when it will be free cannot be known.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use fastctx_hosts::codex::Desk as Use;
use fastctx_types::residents::{Notice, WorldFact};
use fastctx_types::{Address, ResidentId};

use crate::Node;
use crate::speech::{self, Owed};

#[derive(Default)]
pub struct Desk {
    state: Mutex<State>,
}

impl Desk {
    pub fn held_by(&self, resident: &ResidentId) -> bool {
        self.state.lock().expect("never poisoned").holder.as_ref().is_some_and(|(holder, _)| holder == resident)
    }
}

#[derive(Default)]
struct State {
    /// Who sits at the desk, and the turn it sits there for.
    holder: Option<(ResidentId, String)>,
    /// Residents whose Computer Use call started and has not finished.
    calling: HashMap<ResidentId, String>,
    /// Residents refused while a resident of the World sat there, owed word
    /// that the desk is free.
    waiting: BTreeSet<ResidentId>,
}

/// A resident's Computer Use call started, finished, or was refused.
pub fn used(node: &Arc<Node>, resident: &ResidentId, turn: &str, used: Use) {
    let mut tell = Vec::new();
    {
        let mut state = node.desk.state.lock().expect("never poisoned");
        match used {
            Use::Calling => {
                state.calling.insert(resident.clone(), turn.to_string());
            }
            Use::Done => {
                state.calling.remove(resident);
                if state.holder.is_none() {
                    state.holder = Some((resident.clone(), turn.to_string()));
                    state.waiting.remove(resident);
                    sit(node, resident, true);
                }
            }
            Use::Refused => {
                state.calling.remove(resident);
                let other = state
                    .holder
                    .as_ref()
                    .map(|(holder, _)| holder.clone())
                    .filter(|holder| holder != resident)
                    .or_else(|| state.calling.keys().find(|calling| *calling != resident).cloned());
                let fact = match other {
                    Some(other) => {
                        state.waiting.insert(resident.clone());
                        let text = format!(
                            "{other} is using this machine's Computer Use in its turn; you will be told when that turn ends and the desk is free."
                        );
                        (WorldFact::DeskTaken { by: Address { machine: node.machine(), resident: other } }, text)
                    }
                    None => (WorldFact::DeskOutside, "This machine's Computer Use is held by a session fastctx does not know, such as the user's own; when it will be free cannot be known.".to_string()),
                };
                tell.push((resident.clone(), fact));
            }
        }
    }
    send(node, tell, &format!("used:{resident}:{turn}"));
}

/// A resident's turn ended: if it sat at the desk, or its call never
/// finished and nobody else is there, the desk is free.
pub fn turn_ended(node: &Arc<Node>, resident: &ResidentId, turn: &str) {
    let tell = {
        let mut state = node.desk.state.lock().expect("never poisoned");
        let was_calling = state.calling.get(resident).is_some_and(|held| held == turn || turn.is_empty());
        if was_calling {
            state.calling.remove(resident);
        }
        let sat =
            state.holder.as_ref().is_some_and(|(holder, held)| holder == resident && (held == turn || turn.is_empty()));
        if sat {
            state.holder = None;
            sit(node, resident, false);
        }
        let free = sat || (was_calling && state.holder.is_none() && state.calling.is_empty());
        if !free || state.waiting.is_empty() {
            return;
        }
        let text = format!("{resident}'s turn ended: this machine's Computer Use is free.");
        let fact = WorldFact::DeskFree { by: Address { machine: node.machine(), resident: resident.clone() } };
        std::mem::take(&mut state.waiting).into_iter().map(|waiting| (waiting, (fact.clone(), text.clone()))).collect()
    };
    send(node, tell, &format!("ended:{resident}:{turn}"));
}

pub fn removed(node: &Arc<Node>, resident: &ResidentId) {
    node.desk.state.lock().expect("never poisoned").waiting.remove(resident);
}

/// Marks in the resident's `now` whether it sits at the desk.
fn sit(node: &Node, resident: &ResidentId, sits: bool) {
    if let Err(error) = node.residents.change_now(&node.world, resident, |now| {
        now.desk = sits;
        now.desk_since = if sits { Some(jiff::Timestamp::now()) } else { None };
    }) {
        node.log.line(format_args!("recording {resident} at the desk: {error}"));
    }
}

fn send(node: &Arc<Node>, tell: Vec<(ResidentId, (WorldFact, String))>, event: &str) {
    for (resident, (fact, text)) in tell {
        let node = node.clone();
        let id = speech::fixed_id("desk", &[event, resident.as_str(), &text]);
        tokio::spawn(async move {
            let to = Address { machine: node.machine(), resident: resident.clone() };
            let notice = Notice::World { fact, text };
            if let Err(failure) = speech::tell(&node, &Default::default(), &to, id, notice, Owed::Working).await {
                node.log.line(format_args!("telling {resident} about the desk: {failure}"));
            }
        });
    }
}
