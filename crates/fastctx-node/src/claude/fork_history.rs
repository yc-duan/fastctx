//! Forks inherit World turn metadata for the context the host actually copied.

use std::collections::HashSet;
use std::sync::Arc;

use fastctx_hosts::claude::{exchange, transcript};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};

use crate::Node;

pub(super) fn retained(
    node: &Node,
    resident: &ResidentId,
    session: &str,
    through: Option<&str>,
) -> Result<Vec<SessionEvent>, Failure> {
    if !node.residents.knows(resident) {
        return Err(Failure::new("unknown resident"));
    }
    let mut recorded = crate::delivery::history(&node.world.conversation(resident)).map_err(failure)?;
    let removed: HashSet<_> = recorded
        .iter()
        .flat_map(|event| match &event.happened {
            Happened::Changed { change: SessionChange::HistoryCut { removed_turns }, .. } => removed_turns.clone(),
            _ => Vec::new(),
        })
        .collect();
    recorded.retain(|event| turn(&event.happened).is_none_or(|turn| !removed.contains(turn)));
    let directory = exchange::sessions(node.world.path()).join(session);
    let path = exchange::transcript(&directory)
        .map_err(failure)?
        .ok_or_else(|| Failure::new("Claude did not identify the source history file"))?;
    let read =
        transcript::read_with_cuts(&path, &super::rewind::cuts(&directory).map_err(failure)?).map_err(failure)?;
    if !read.problems.is_empty() {
        return Err(Failure::new(read.problems.join("; ")));
    }
    let context: Vec<_> = read
        .context_times
        .iter()
        .map(|&at| SessionEvent {
            at,
            happened: Happened::Heard {
                delivery: None,
                turn: None,
                from: None,
                text: String::new(),
                message: None,
                notice: None,
            },
        })
        .collect();
    let retained: HashSet<_> = super::history::place(context, &recorded)
        .iter()
        .filter_map(|event| turn(&event.happened).map(str::to_owned))
        .collect();
    let mut beyond = false;
    let mut allowed = HashSet::new();
    for event in &recorded {
        if let Happened::TurnStarted { turn } = &event.happened {
            if !beyond && retained.contains(turn) {
                allowed.insert(turn.clone());
            }
            if Some(turn.as_str()) == through {
                beyond = true;
            }
        }
    }
    if through.is_some() && !beyond {
        return Err(Failure::new("the World has not retained the selected turn's metadata"));
    }
    Ok(recorded
        .into_iter()
        .filter(|event| {
            turn(&event.happened).is_some_and(|turn| allowed.contains(turn))
                && match &event.happened {
                    Happened::Heard { text, .. } => read.context_messages.iter().any(|message| {
                        message["role"] == "user"
                            && message["text"].as_str().is_some_and(|stored| {
                                stored == text || stored.contains(&fastctx_hosts::claude::framed(text))
                            })
                    }),
                    _ => true,
                }
        })
        .map(|mut event| {
            if let Happened::TurnEnded { end, .. } = &mut event.happened
                && let Some(answer) = &end.last_message
                && !read
                    .context_messages
                    .iter()
                    .any(|message| message["role"] == "assistant" && message["text"].as_str() == Some(answer.as_str()))
            {
                end.last_message = None;
            }
            event
        })
        .collect())
}

pub(super) fn seed(node: &Arc<Node>, resident: &ResidentId, recorded: &[SessionEvent]) -> Result<(), Failure> {
    if !node.residents.knows(resident) {
        return Err(Failure::new("unknown fork resident"));
    }
    let directory = node.world.conversation(resident);
    for event in recorded {
        let key = serde_json::to_string(event).map_err(|error| Failure::new(error.to_string()))?;
        crate::delivery::event(&directory, &format!("inherited:{key}"), event).map_err(failure)?;
    }
    let last = recorded.iter().rev().find_map(|event| match &event.happened {
        Happened::TurnEnded { end, .. } => end.last_message.as_deref(),
        _ => None,
    });
    let first = recorded.iter().find_map(|event| match &event.happened {
        Happened::Heard { from: Some(fastctx_types::session::Speaker::User { .. }), text, .. } => Some(text.as_str()),
        _ => None,
    });
    node.residents
        .change_now(&node.world, resident, |now| {
            now.last_said = last.map(|text| crate::turns::opening(text, fastctx_types::residents::LAST_SAID_CHARS));
            now.first_words =
                first.map(|text| crate::turns::opening(text, fastctx_types::residents::FIRST_WORDS_CHARS));
        })
        .map_err(failure)
}

fn turn(happened: &Happened) -> Option<&str> {
    match happened {
        Happened::TurnStarted { turn } | Happened::TurnEnded { turn, .. } => Some(turn),
        Happened::Heard { turn: Some(turn), .. } => Some(turn),
        Happened::Changed {
            turn: Some(turn),
            change: SessionChange::ModelRequest { .. } | SessionChange::ModelResponse { .. },
        } => Some(turn),
        _ => None,
    }
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
