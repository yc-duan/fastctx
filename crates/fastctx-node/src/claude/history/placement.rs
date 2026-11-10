//! Places host events in recorded turns without scanning all turns per event.

use super::hidden;
use fastctx_hosts::claude;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

/// Puts each event of Claude Code's record in the World's turn it happened
/// in: the turn running at its time, and for words heard just before a turn
/// started with nothing else between, that turn, as they are what started
/// it. What the World did not see stays where it is, in no turn. Words the
/// World itself handed over are told by the World's record, which knows
/// who said them, and are left out here.
pub(crate) fn place(mut events: Vec<SessionEvent>, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
    let mut turns: Vec<(Timestamp, Option<Timestamp>, String)> = Vec::new();
    let mut ends: HashMap<&str, Timestamp> = HashMap::new();
    let mut heard: HashMap<&str, Vec<&str>> = HashMap::new();
    for event in recorded {
        match &event.happened {
            Happened::TurnEnded { turn, .. } => {
                ends.insert(turn, event.at);
            }
            Happened::Heard { turn: Some(turn), text, .. } => heard.entry(turn).or_default().push(text),
            _ => {}
        }
    }
    for event in recorded {
        if let Happened::TurnStarted { turn } = &event.happened {
            turns.push((event.at, ends.get(turn.as_str()).copied(), turn.clone()));
        }
    }
    turns.sort_by_key(|(start, ..)| *start);
    events.sort_by_key(|event| event.at);
    let mut next_visible = vec![None; events.len()];
    let mut next = None;
    for (index, event) in events.iter().enumerate().rev() {
        next_visible[index] = next;
        if !hidden(event) {
            next = Some(event.at);
        }
    }
    let mut started = 0;
    let mut active = BTreeSet::new();
    let mut ending = BinaryHeap::new();
    let mut placed = Vec::with_capacity(events.len());
    for (index, mut event) in events.into_iter().enumerate() {
        while started < turns.len() && turns[started].0 <= event.at {
            active.insert(started);
            if let Some(end) = turns[started].1 {
                ending.push(Reverse((end, started)));
            }
            started += 1;
        }
        while ending.peek().is_some_and(|Reverse((end, _))| *end < event.at) {
            let Reverse((_, index)) = ending.pop().expect("ended turn");
            active.remove(&index);
        }
        let turn = active.last().map(|index| turns[*index].2.clone()).or_else(|| match event.happened {
            Happened::Heard { .. } => {
                let (start, _, turn) = turns.get(turns.partition_point(|(start, ..)| *start < event.at))?;
                // What the host keeps out of its conversation, such as the
                // context it gives the model with the words, is not between.
                next_visible[index].is_none_or(|next| next >= *start).then(|| turn.clone())
            }
            _ => None,
        });
        if let Some(turn) = turn {
            match &mut event.happened {
                Happened::Heard { turn: slot, .. }
                | Happened::Health { turn: slot, .. }
                | Happened::Changed { turn: slot, .. } => *slot = Some(turn),
                Happened::Began { turn: slot, .. }
                | Happened::Grew { turn: slot, .. }
                | Happened::Finished { turn: slot, .. }
                | Happened::Steps { turn: slot, .. } => *slot = turn,
                _ => {}
            }
        }
        // Words the World's telling went in beside are the speaker's alone.
        if let Happened::Heard { text, .. } = &mut event.happened
            && let Some(words) = claude::beside_framed(text)
        {
            *text = words.to_string();
        }
        if let Happened::Heard { turn, text, message, .. } = &event.happened {
            let told = claude::is_framed(text)
                || turn.as_deref().and_then(|turn| heard.get(turn)).is_some_and(|said| said.contains(&text.as_str()));
            if told
                && fastctx_types::session::attachments::Envelope::read(text).is_none()
                && message.as_ref().is_none_or(|message| message.images.is_empty())
            {
                continue;
            }
        }
        placed.push(event);
    }
    placed
}
