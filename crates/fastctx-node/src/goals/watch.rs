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
    let account = super::scope::resolve(node, resident, Some(account))?;
    let events = node.watchers.watch(resident);
    let view =
        super::operations::read(node, &Address { machine: node.machine(), resident: resident.clone() }, &account)
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
