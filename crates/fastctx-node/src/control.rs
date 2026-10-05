//! What the user does to a conversation besides speaking into it, and what
//! a conversation offers (`fastctx_types::session::control`). The call goes
//! to the machine the resident lives on, as a stop does, and that machine
//! asks the resident's host: a Codex thread of its own app-server through
//! the app-server, a Claude session through the World's mod.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::{Address, MachineName, ResidentId};

use crate::Node;

pub async fn control(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
    control: Control,
) -> Result<(Address, Controlled), Failure> {
    let machine = machine_of(node, &resident, machine).await?;
    if machine == node.machine() {
        let controlled = control_here(node, &resident, control).await?;
        return Ok((Address { machine, resident }, controlled));
    }
    let call = RemoteCall::Control { resident: resident.clone(), control };
    match link(node)?.call(&machine, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Controlled(controlled)) => Ok((Address { machine, resident }, controlled)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than what was done"))),
        Err(failure) => Err(failure),
    }
}

pub async fn offers(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
) -> Result<(Address, Offers), Failure> {
    let machine = machine_of(node, &resident, machine).await?;
    if machine == node.machine() {
        let offers = offers_here(node, &resident).await?;
        return Ok((Address { machine, resident }, offers));
    }
    let call = RemoteCall::Offers { resident: resident.clone() };
    match link(node)?.call(&machine, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Offers(offers)) => Ok((Address { machine, resident }, *offers)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than what it offers"))),
        Err(failure) => Err(failure),
    }
}

/// Does `control` to a resident of this machine, through its host. Sessions
/// the World does not hold (the user's own Codex in an app) take nothing.
pub async fn control_here(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    let host = host_of(node, resident)?;
    if host == "Claude" {
        return crate::claude::control::control(node, resident, control).await;
    }
    let _ = control;
    Err(Failure::new(format!("{host} sessions take nothing but words yet")))
}

/// What a resident of this machine offers now, as its host says.
pub async fn offers_here(node: &Arc<Node>, resident: &ResidentId) -> Result<Offers, Failure> {
    if host_of(node, resident)? == "Claude" {
        return crate::claude::control::offers(node, resident).await;
    }
    Ok(Offers::default())
}

/// The host a resident of this machine is held through, by name.
fn host_of(node: &Node, resident: &ResidentId) -> Result<&'static str, Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    match (about.host.agent.as_str(), about.world_started, &about.session) {
        ("codex", true, Some(_)) => Ok("Codex"),
        ("claude", _, _) => Ok("Claude"),
        _ => Err(Failure::new(format!("{resident} is a session the World does not hold; do this where it is open"))),
    }
}

async fn machine_of(
    node: &Arc<Node>,
    resident: &ResidentId,
    machine: Option<MachineName>,
) -> Result<MachineName, Failure> {
    match machine {
        Some(machine) => Ok(machine),
        None if node.residents.about(resident).is_some() => Ok(node.machine()),
        None => crate::colleagues::find(node, resident).await,
    }
}

fn link(node: &Node) -> Result<Arc<fastctx_net::link::Link>, Failure> {
    node.link().ok_or_else(|| Failure::new(format!("{} is in no World with other machines", node.machine())))
}
