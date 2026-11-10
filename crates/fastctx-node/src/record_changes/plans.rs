//! Plan checks use the account already selected at the entrance.
use crate::{Node, town::caller::Caller};
use fastctx_types::{Call, ipc::Failure};
use std::sync::Arc;

pub(super) fn parameters(node: &Node, caller: &Caller, call: &Call) -> Result<(), Failure> {
    match call {
        Call::Plan { .. } => {
            let Caller::Resident(address) = caller else {
                return Err(Failure::not_run(fastctx_types::plan::words::RESIDENT));
            };
            if node.is_me(&address.machine) {
                node.residents.active(&address.resident)?;
            }
        }
        Call::PlanWrite { markdown, .. } => {
            if !matches!(caller, Caller::User) {
                return Err(Failure::refused("only the user writes a plan through this call"));
            }
            fastctx_types::plan::validate(markdown).map_err(Failure::not_run)?;
        }
        _ => return Err(Failure::not_run("this is not a plan operation")),
    }
    Ok(())
}

pub(super) fn check(
    node: &Arc<Node>,
    caller: &Caller,
    call: &Call,
    resolved: &super::Resolved,
) -> Result<String, Failure> {
    match call {
        Call::Plan { .. } => {
            let Caller::Resident(address) = caller else {
                return Err(Failure::not_run(fastctx_types::plan::words::RESIDENT));
            };
            if node.is_me(&address.machine) {
                node.residents.active(&address.resident)?;
            }
            super::unit(&("conversation", &address.resident))
        }
        Call::PlanWrite { owner, markdown, machine, .. } => {
            if !matches!(caller, Caller::User) {
                return Err(Failure::refused("only the user writes a plan through this call"));
            }
            fastctx_types::plan::validate(markdown).map_err(Failure::not_run)?;
            let super::Resolved::PlanWrite { scope } = resolved else {
                return Err(Failure::not_run("the plan has no selected account"));
            };
            super::unit(&("plan", scope, machine, owner))
        }
        _ => Err(Failure::not_run("this is not a plan operation")),
    }
}
