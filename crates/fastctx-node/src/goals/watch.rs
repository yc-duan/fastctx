//! A scoped goal watch subscribes before reading and never loads the model's transcript.

use crate::Node;
use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, Watched};
use fastctx_types::{Address, ResidentId};
use std::sync::Arc;

pub(crate) async fn opening(
    node: &Arc<Node>,
    resident: &ResidentId,
    account: &HubAccount,
) -> Result<crate::talk::Opening, Failure> {
    let actor = account.clone();
    let account = super::scope::resolve(node, resident, Some(account))?;
    let events = node.watchers.watch(resident);
    let conversation = Address { machine: node.machine(), resident: resident.clone() };
    let access = crate::record_access::Access { actor, owner: account.clone(), conversation: conversation.clone() };
    let modern = node.link_of(&account.hub).is_some_and(|link| link.view().shared_records);
    let view =
        crate::record_access::within(modern.then_some(access), super::operations::read(node, &conversation, &account))
            .await?;
    let event = SessionEvent {
        at: jiff::Timestamp::now(),
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Goal { goal: serde_json::json!({"world":true,"view":view}) },
        },
    };
    Ok(crate::talk::Opening { pieces: vec![Watched::Live { event }], events, slim: None })
}

pub(crate) fn is_goal(event: &SessionEvent) -> bool {
    matches!(&event.happened, Happened::Changed { change: SessionChange::Goal { goal }, .. } if goal["world"] == true)
}
