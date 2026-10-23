//! A conversation may stop the current turn of a colleague it asked for.

use crate::{Node, town::caller::Caller};
use fastctx_types::{
    Address, Reply, ResidentId,
    access::{Purpose, Written},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
};
use std::sync::Arc;

pub(crate) async fn stop(
    node: &Arc<Node>,
    parent: Address,
    id: ResidentId,
    machine: Option<Written>,
) -> Result<Reply, Failure> {
    let acting = crate::around::acting_of(node, &Caller::Resident(parent.clone()));
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.knows(&id) => node.machine().into(),
        None => super::find_for(node, &acting, &id).await?,
    };
    let origin = if node.is_here(&machine) {
        if !crate::admission::seen::resident(node, crate::admission::seen::Looker::Here(&acting), &id) {
            return Err(Failure::refused(fastctx_types::collab_prompt::NOT_YOUR_COLLEAGUE));
        }
        node.residents.about(&id).ok_or_else(|| crate::forget::gone(node, &id))?.0.origin
    } else {
        let reply = node
            .call(&acting, &machine, Purpose::Look, RemoteCall::Who { all: true, town: None, since: None })
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
        let RemoteReply::Who(who) = reply else {
            return Err(Failure::new(fastctx_types::collab_prompt::STOP_IDENTIFY));
        };
        who.residents
            .into_iter()
            .find(|resident| resident.about.id == id)
            .ok_or_else(|| Failure::refused(fastctx_types::collab_prompt::NOT_YOUR_COLLEAGUE))?
            .about
            .origin
    };
    let Origin::Colleague { asked_by, .. } = origin else {
        return Err(Failure::refused(fastctx_types::collab_prompt::NOT_YOUR_COLLEAGUE));
    };
    let same_machine = parent.machine == asked_by.machine
        || (node.is_me(&parent.machine) && node.is_me(&asked_by.machine))
        || node.links().iter().any(|link| link.view().same(&parent.machine, &asked_by.machine));
    if parent.resident != asked_by.resident || !same_machine {
        return Err(Failure::refused(fastctx_types::collab_prompt::NOT_YOUR_COLLEAGUE));
    }
    let stopped = if node.is_here(&machine) {
        crate::talk::stop_here(node, &id).await?
    } else {
        let reply = node
            .call(&acting, &machine, Purpose::Agents, RemoteCall::Stop { resident: id.clone() })
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
        let RemoteReply::Stopped { stopped } = reply else {
            return Err(Failure::new(fastctx_types::collab_prompt::STOP_UNANSWERED));
        };
        stopped
    };
    Ok(Reply::Stopped { resident: Address { machine: machine.name, resident: id }, stopped })
}
