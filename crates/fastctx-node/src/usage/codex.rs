//! Host-calculated per-thread deltas, including child and ephemeral title threads.
use super::ledger::Entry;
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnTokens};
use fastctx_types::usage_calls::{CallOutcome, CallPhase, CallSource, CallTiming};
use serde::Deserialize;
use std::io;
use std::path::Path;

#[derive(Deserialize)]
struct Counted {
    thread: String,
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    tokens: Option<TurnTokens>,
    #[serde(default)]
    turn: Option<String>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    request: Option<Request>,
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    sequence: Option<u64>,
    timing: CallTiming,
    outcome: CallOutcome,
    phase: CallPhase,
}

pub(super) fn measured(
    node: &Node,
    path: &Path,
    id: &ResidentId,
    about: &About,
    event: &SessionEvent,
) -> io::Result<bool> {
    let mut happened = &event.happened;
    let mut agent = None;
    while let Happened::Changed { change: SessionChange::AgentEvent { agent_id: child, happened: inner, .. }, .. } =
        happened
    {
        agent = Some(child.clone());
        happened = inner;
    }
    let Happened::Changed { turn, change: SessionChange::Usage { usage } } = happened else {
        return Ok(false);
    };
    let Some(counted) = usage.get("counted") else {
        return Ok(false);
    };
    let counted: Counted = serde_json::from_value(counted.clone()).map_err(io::Error::other)?;
    let tokens = counted.tokens.as_ref().map(super::tokens_of).unwrap_or_default();
    if tokens.total() == 0 && counted.request.is_none() {
        return Ok(false);
    }
    let key = match counted.request.as_ref().and_then(|r| r.sequence) {
        Some(sequence) => format!("{}:{}:{sequence}", counted.thread, event.at),
        None => format!("{}:{}", counted.thread, event.at),
    };
    super::with_book(node, path, |book| {
        if book.keys.contains(&key) {
            return Ok(false);
        }
        book.commit(
            path,
            Entry::Tokens {
                at: event.at,
                who: super::who(about),
                with: super::with(about),
                model: counted.model.or_else(|| about.model.clone()).unwrap_or_default(),
                tokens,
                key: Some(key),
                call: Some(super::calls::Detail {
                    conversation: id.clone(),
                    acting: about.acting.clone(),
                    turn: counted.turn.or_else(|| turn.clone()),
                    source: if counted.background {
                        CallSource::Background
                    } else if agent.is_some() {
                        CallSource::Agent
                    } else {
                        CallSource::Main
                    },
                    agent,
                    effort: counted.effort,
                    timing: counted.request.as_ref().map(|r| r.timing.clone()).unwrap_or(CallTiming {
                        started_at: None,
                        first_at: None,
                        ended_at: event.at,
                        approximate: true,
                    }),
                    outcome: counted.request.as_ref().map_or(CallOutcome::Completed, |r| r.outcome),
                    phase: counted.request.as_ref().map_or(CallPhase::Finished, |r| r.phase),
                    observed: counted.tokens.is_some(),
                }),
            },
        )?;
        Ok(true)
    })
}
