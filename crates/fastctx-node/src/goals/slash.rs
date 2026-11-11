//! `/goal` from a conversation's control queue (`Control::Goal`), done in its
//! order among the words the user queued, as every slash command is: words
//! for a goal still open replace its words; otherwise they set a new one, as
//! the side panel does.

use std::sync::Arc;

use fastctx_types::goals::{Author, Request, Status, Updated, UserChange};
use fastctx_types::ipc::Failure;
use fastctx_types::session::control::Controlled;
use fastctx_types::{Address, ResidentId};

use crate::Node;

/// Sets the goal of `resident` for the user. The caller holds the
/// conversation's action gate, as the control queue does.
pub(crate) async fn set(node: &Arc<Node>, resident: &ResidentId, objective: &str) -> Result<Controlled, Failure> {
    if !node.goals.records.available() {
        return Err(Failure::not_run(super::words::RECORDS_MISSING));
    }
    if !node.goals.context.available() {
        return Err(Failure::not_run(super::words::CONTEXT_MISSING));
    }
    let objective = objective.trim();
    if objective.is_empty() {
        return Err(Failure::refused("/goal needs the goal's words"));
    }
    let account = super::scope::user(node, resident, None).await?;
    let address = Address { machine: node.machine(), resident: resident.clone() };
    let _place = node.places.using(resident.as_str()).await;
    let view = super::operations::read(node, &address, &account).await?;
    let change = match &view.goal {
        Some(goal) if goal.status != Status::Complete => UserChange::Edit { objective: objective.into() },
        _ => {
            if node.residents.about(resident).is_some_and(|(about, _)| !about.world_managed()) {
                return Err(Failure::refused(super::words::NATIVE_CODEX));
            }
            UserChange::Open { objective: objective.into(), standing: false }
        }
    };
    let operation = crate::speech::new_id("goal").to_string();
    let request = Request::Change { operation, expected: view.version, change };
    let author = Author::User { machine: node.machine() };
    match super::operations::call_held(node, address, account, author, None, request).await? {
        Updated::Conflict { .. } => Err(Failure::new("the goal changed while /goal waited; set it again")),
        _ => Ok(Controlled::Done { said: None }),
    }
}
