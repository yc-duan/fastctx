//! A late-bound user stop clears only the trouble produced by that terminal.
use crate::Node;
use fastctx_types::{ResidentId, residents::Now, session::TurnHow};
use std::io;

pub(super) fn user(
    node: &Node,
    resident: &ResidentId,
    turn: &str,
    finished: &crate::delivery::Finished,
) -> io::Result<()> {
    if !matches!(finished.end.how, TurnHow::Interrupted) {
        return Ok(());
    }
    let expected = match &finished.end.reason {
        Some(reason) => format!("turn {}: {reason}", finished.end.how),
        None => format!("turn {}", finished.end.how),
    };
    let matches = |now: &Now| {
        now.turn.is_none()
            && now.last_turn.as_deref() == Some(turn)
            && now.trouble.as_ref().is_some_and(|trouble| trouble.at == finished.at && trouble.text == expected)
    };
    if node.residents.now(resident).is_some_and(|now| matches(&now)) {
        node.residents.change_now(&node.world, resident, |now| {
            if matches(now) {
                now.trouble = None;
            }
        })?;
    }
    Ok(())
}
