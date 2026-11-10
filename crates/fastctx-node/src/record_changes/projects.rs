//! Reject malformed project changes before starting external work.
use crate::{Node, town::caller::Caller};
use fastctx_types::{ProjectCall, ipc::Failure, project::RecordFields};
use std::sync::Arc;

pub(super) fn check(node: &Arc<Node>, caller: &Caller, call: &ProjectCall) -> Result<String, Failure> {
    match call {
        ProjectCall::Enter { .. } => {
            let Caller::Resident(address) = caller else {
                return Err(Failure::not_run("only an agent's resident conversation enters a project"));
            };
            if node.is_me(&address.machine) {
                node.residents.active(&address.resident)?;
            }
            super::unit(&("conversation", &address.resident))
        }
        ProjectCall::New { town, fields, account, hub } => {
            fields_check(node, caller, fields)?;
            if fields.name.is_none() {
                return Err(Failure::new("a new project needs its name (--name)"));
            }
            if fields.what.is_none() {
                return Err(Failure::new("a new project needs to say what it is (--what)"));
            }
            crate::town::project::owner(node, caller, account.clone(), hub.clone())?;
            super::unit(&("project", town))
        }
        ProjectCall::Edit { town, fields } => {
            fields_check(node, caller, fields)?;
            if fields == &RecordFields::default() {
                return Err(Failure::new("nothing to change: give what the next version says"));
            }
            let town = super::town(node, caller, town.as_ref())?;
            super::unit(&("project", town))
        }
        _ => Err(Failure::not_run("this project call is not a change")),
    }
}
fn fields_check(node: &Node, caller: &Caller, fields: &RecordFields) -> Result<(), Failure> {
    if fields.canon.is_some() {
        return Err(Failure::not_run(fastctx_types::memory::LEGACY));
    }
    if !matches!(caller, Caller::User | Caller::Resident(_)) {
        return Err(Failure::refused("only a user or a conversation writes history"));
    }
    if fields.machines.as_ref().is_some_and(Vec::is_empty) {
        return Err(Failure::new("a project spans at least one machine"));
    }
    for place in fields.places.iter().flatten() {
        let path = if node.is_me(&place.location.machine) {
            fastctx_platform::local_path(&place.location.path)
                .map_err(|error| Failure::new(error.to_string()))?
                .to_string_lossy()
                .into_owned()
        } else {
            place.location.path.clone()
        };
        crate::town::project::absolute(&path).map_err(Failure::new)?;
        if place.what.trim().is_empty() {
            return Err(Failure::new(format!("the place {} says nothing of what is there", place.location.path)));
        }
    }
    Ok(())
}
