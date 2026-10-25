//! A permanent preflight refusal is durable without claiming host acceptance.

use crate::Node;
use fastctx_types::{
    NoticeId, ResidentId,
    ipc::Failure,
    residents::{Landed, Pending},
    session::{Happened, Health, HealthKind, SessionEvent},
};
use serde::{Deserialize, Serialize};
use std::{io, sync::Arc};

#[derive(Serialize, Deserialize)]
struct Refused {
    pending: Pending,
    failure: Failure,
    at: jiff::Timestamp,
}

fn path(node: &Node, id: &ResidentId, input: &NoticeId) -> std::path::PathBuf {
    node.world.conversation(id).book("host-refusals").join(format!("{input}.json"))
}

pub(crate) fn get(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Failure>> {
    fastctx_hosts::claude::control::read::<Refused>(&path(node, id, input))
        .map(|row| row.map(|row| marked(row.failure)))
}

/// The caller proves that no host command capable of consuming this input ran.
/// Keep the words and refusal first; an interrupted cleanup repeats from this record.
pub(crate) async fn reject(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    error: Failure,
) -> Result<Landed, Failure> {
    let directory = node.world.conversation(id);
    let record = path(node, id, input);
    let refused = match fastctx_hosts::claude::control::read::<Refused>(&record).map_err(failure)? {
        Some(refused) => refused,
        None => {
            let Some(pending) = crate::inbox::read(&directory, input).map_err(failure)? else { return Err(error) };
            let refused = Refused { pending, failure: error, at: jiff::Timestamp::now() };
            fastctx_hosts::claude::control::write(&record, &refused).map_err(failure)?;
            refused
        }
    };
    let text = format!("{} {input}: {}", super::words::NOT_SENT, refused.failure.message);
    let event = SessionEvent {
        at: refused.at,
        happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Error, text: text.clone() } },
    };
    if crate::delivery::event(&directory, &format!("host-refused:{input}"), &event).map_err(failure)? {
        node.watchers.publish(id, &event);
    }
    crate::composer::rejected(node, id, input, &refused.failure.message).await?;
    node.residents
        .change_now(&node.world, id, |now| {
            now.trouble = Some(fastctx_types::residents::Trouble { at: refused.at, text });
        })
        .map_err(failure)?;
    crate::inbox::set_state(&directory, input, fastctx_types::residents::PendingState::Waiting).map_err(failure)?;
    crate::inbox::withdraw(&directory, input).map_err(failure)?;
    super::retry::clear(node, id, input).map_err(failure)?;
    Err(marked(refused.failure))
}

/// The destination recorded a final preflight failure using the existing told contract.
pub(crate) fn reported(error: &Failure) -> bool {
    error.not_run && error.code == Some(fastctx_types::ipc::FailureCode::Told)
}

fn marked(mut error: Failure) -> Failure {
    error.not_run = true;
    error.code = Some(fastctx_types::ipc::FailureCode::Told);
    error
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
