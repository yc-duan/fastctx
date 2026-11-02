//! A permanent preflight refusal is durable without claiming host acceptance.
//! There is one for each input a host refused for good, kept as long as the
//! conversation is.

use crate::Node;
use fastctx_types::{
    NoticeId, ResidentId,
    ipc::Failure,
    residents::{Landed, Pending},
    session::{Happened, Health, HealthKind, SessionEvent},
};
use serde::{Deserialize, Serialize};
use std::{io, sync::Arc};

#[derive(Clone, Serialize, Deserialize)]
struct Refused {
    pending: Pending,
    failure: Failure,
    at: jiff::Timestamp,
}

pub(crate) fn prepare(directory: &fastctx_types::layout::ConversationDir) -> io::Result<()> {
    crate::records::legacy::directory::<Refused>(directory, "host-refusals", &directory.book("host-refusals"))
}
pub(crate) fn contains(directory: &fastctx_types::layout::ConversationDir, input: &NoticeId) -> io::Result<bool> {
    prepare(directory)?;
    crate::records::read(directory, |rows| Ok(rows.get::<Refused>("host-refusals", input.as_str())?.is_some()))
}
pub(crate) fn get(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Failure>> {
    let directory = node.world.conversation(id);
    prepare(&directory)?;
    crate::records::read(&directory, |rows| {
        Ok(rows.get::<Refused>("host-refusals", input.as_str())?.map(|row| marked(row.failure)))
    })
}

/// The caller proves that no host command capable of consuming this input ran.
/// Keep the words and refusal first; an interrupted cleanup repeats from this record.
pub(crate) async fn reject(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    error: Failure,
) -> Result<Landed, Failure> {
    let (saving, recipient, key, offered) = (node.clone(), id.clone(), input.clone(), error.clone());
    let kept = fastctx_platform::task::spawn_blocking(move || -> io::Result<Option<Refused>> {
        let directory = saving.world.conversation(&recipient);
        prepare(&directory)?;
        crate::inbox::storage::initialize(&directory)?;
        crate::session_record::prepare(&directory)?;
        let saved = crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 2048, move |rows| {
            let refused = match rows.get::<Refused>("host-refusals", key.as_str())? {
                Some(row) => row,
                None => {
                    let Some(pending) = rows.get::<Pending>("pending-inputs", key.as_str())? else { return Ok(None) };
                    Refused { pending, failure: offered, at: jiff::Timestamp::now() }
                }
            };
            let text = format!("{} {key}: {}", super::words::NOT_SENT, refused.failure.message);
            let event = SessionEvent {
                at: refused.at,
                happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Error, text } },
            };
            rows.put("host-refusals", key.as_str(), &refused)?;
            let changed = rows.event(&format!("host-refused:{key}"), &event)?;
            rows.delete("pending-inputs", key.as_str())?;
            Ok(Some((refused, event, changed)))
        })?;
        if let Some((refused, event, changed)) = saved {
            if changed {
                saving.watchers.publish(&recipient, &event);
            }
            Ok(Some(refused))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    let Some(refused) = kept else { return Err(error) };
    crate::composer::rejected(node, id, input, &refused.failure.message).await?;
    let (saving, recipient, key, result) = (node.clone(), id.clone(), input.clone(), refused.clone());
    fastctx_platform::task::spawn_blocking(move || -> io::Result<()> {
        let text = format!("{} {key}: {}", super::words::NOT_SENT, result.failure.message);
        saving.residents.change_now(&saving.world, &recipient, |now| {
            now.trouble = Some(fastctx_types::residents::Trouble { at: result.at, text });
        })?;
        super::retry::clear(&saving, &recipient, &key)
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
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
