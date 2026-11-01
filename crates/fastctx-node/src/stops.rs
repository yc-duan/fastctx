//! A stop follows the exact inputs it was requested for, across restarts.
//! Once a consuming turn is known, only that turn may receive the stop.

use crate::Node;
use fastctx_types::{NoticeId, ResidentId, ipc::Failure, turn_evidence::StopSource};
use std::{io, sync::Arc, time::Duration};

#[derive(Default)]
pub(crate) struct Stops {
    recovering: std::sync::Mutex<std::collections::HashMap<ResidentId, bool>>,
}

mod recovery;
mod settled;
mod store;

pub(crate) fn requested_turn(node: &Node, resident: &ResidentId, turn: &str) -> io::Result<bool> {
    if crate::turn_evidence::source(node, resident, turn)? == Some(StopSource::User) {
        return Ok(true);
    }
    let key = crate::speech::fixed_id("stopped", &[turn]);
    crate::records::metadata::exists(&node.world.conversation(resident).book("user-stops").join(key.as_str()))
}

impl Stops {
    /// A start publishes Now before reading these markers. The shared ordering
    /// permit makes that read wait for a stop that observed an unbound turn.
    pub(crate) async fn coming(
        &self,
        node: &Arc<Node>,
        resident: &ResidentId,
        coming: Vec<NoticeId>,
        source: StopSource,
    ) -> io::Result<bool> {
        let committed = recovery::notification(node, resident);
        let (node, resident) = (node.clone(), resident.clone());
        store::ordered(node.world.conversation(&resident), move |directory, order| {
            if coming.is_empty() || node.residents.now(&resident).is_some_and(|now| now.turn.is_some()) {
                return Ok(false);
            }
            store::keep(directory, &coming, source, order, &committed)?;
            Ok(true)
        })
        .await
    }
    pub(crate) async fn started<'a>(
        &self,
        node: &Arc<Node>,
        resident: &ResidentId,
        words: impl IntoIterator<Item = &'a NoticeId>,
    ) -> io::Result<Option<StopSource>> {
        let words = words.into_iter().cloned().collect::<Vec<_>>();
        store::ordered(node.world.conversation(resident), move |directory, _| store::source(directory, &words)).await
    }
}

/// Stop delivery is independent of the report consumer and never blocks it.
pub(crate) fn deliver(node: &Arc<Node>, resident: &ResidentId, turn: &str, inputs: Vec<NoticeId>) {
    let (node, resident, turn) = (node.clone(), resident.clone(), turn.to_string());
    tokio::spawn(async move {
        let mut problem = None;
        loop {
            if node.residents.about(&resident).is_none() {
                return;
            }
            match apply(&node, &resident, &turn, &inputs).await {
                Ok(()) => return,
                Err(error) => {
                    if problem.as_ref() != Some(&error.message) {
                        node.log.line(format_args!("stopping {resident}'s consuming turn {turn}; retrying: {error}"));
                        problem = Some(error.message);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

async fn apply(node: &Arc<Node>, resident: &ResidentId, turn: &str, inputs: &[NoticeId]) -> Result<(), Failure> {
    let Some(source) = node.stops.started(node, resident, inputs).await.map_err(failure)? else {
        return Ok(());
    };
    let directory = node.world.conversation(resident);
    crate::turn_evidence::remember(node, resident, turn, source).await.map_err(failure)?;
    let (reading, requested) = (directory.clone(), turn.to_string());
    let ended = tokio::task::spawn_blocking(move || crate::delivery::terminal(&reading, &requested))
        .await
        .map_err(failure)?
        .map_err(failure)?;
    if let Some(finished) = ended {
        if source == StopSource::User {
            let (node, resident, turn) = (node.clone(), resident.clone(), turn.to_string());
            tokio::task::spawn_blocking(move || settled::user(&node, &resident, &turn, &finished))
                .await
                .map_err(failure)?
                .map_err(failure)?;
        }
    } else {
        let speaking = node.residents.speaking(resident).ok_or_else(crate::authorization::no_subject)?;
        let guard = speaking.lock().await;
        if node.residents.now(resident).and_then(|now| now.turn).as_deref() != Some(turn) {
            return Err(Failure::new("the consuming turn has not become current or settled yet"));
        }
        node.interrupts.raise(&fastctx_types::Address { machine: node.machine(), resident: resident.clone() });
        drop(guard);
        let (about, _) = node.residents.about(resident).ok_or_else(crate::authorization::no_subject)?;
        let session = about.session.as_deref().ok_or_else(|| Failure::new("the consuming session is unknown"))?;
        match about.host.agent.as_str() {
            "codex" => {
                node.codex.of(node, &about)?.interrupt_turn(session, turn).await.map(|_| ()).map_err(Failure::new)?
            }
            "claude" => {
                let exchange = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
                let turn = turn.to_string();
                tokio::task::spawn_blocking(move || fastctx_hosts::claude::exchange::stop(&exchange, &turn))
                    .await
                    .map_err(failure)?
                    .map_err(failure)?;
            }
            _ => return Err(Failure::new("the consuming host has no stop entrance")),
        }
    }
    let inputs = inputs.to_vec();
    if !store::ordered(directory, move |directory, order| store::finish(directory, &inputs, source, order))
        .await
        .map_err(failure)?
    {
        return Err(Failure::new("a stronger stop arrived while the host was interrupted"));
    }
    Ok(())
}

pub(crate) fn recover(node: &Arc<Node>) {
    let runtime = tokio::runtime::Handle::current();
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        recovery::schedule(&runtime, node, &resident.id);
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping a stop for its input: {error}"))
}
