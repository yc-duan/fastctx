//! Plan reads also publish what the conversation has observed.
use crate::{Node, town::caller::Caller};
use fastctx_types::{Call, ipc::Failure};
use std::sync::Arc;

pub(super) fn check(node: &Arc<Node>, caller: &Caller, call: &Call) -> Result<String, Failure> {
    match call {
        Call::Plan { hub, account, .. } => {
            let Caller::Resident(address) = caller else {
                return Err(Failure::not_run(fastctx_types::plan::words::RESIDENT));
            };
            if node.is_me(&address.machine) {
                node.residents.active(&address.resident)?;
            }
            crate::profile::scope(node, caller, hub.as_ref(), account.as_ref())?;
            super::unit(&("conversation", &address.resident))
        }
        Call::PlanWrite { hub, account, owner, markdown, machine, .. } => {
            if !matches!(caller, Caller::User) {
                return Err(Failure::refused("only the user writes a plan through this call"));
            }
            fastctx_types::plan::validate(markdown).map_err(Failure::not_run)?;
            crate::profile::scope(node, caller, hub.as_ref(), account.as_ref())?;
            super::unit(&("plan", hub, account, machine, owner))
        }
        _ => Err(Failure::not_run("this is not a plan operation")),
    }
}
