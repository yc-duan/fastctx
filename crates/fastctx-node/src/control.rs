//! What the user does to a conversation besides speaking into it, and what
//! a conversation offers (`fastctx_types::session::control`). The call goes
//! to the machine the resident lives on, as a stop does, and that machine
//! asks the resident's host: a Codex thread of its own app-server through
//! the app-server, a Claude session through the World's mod.

use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
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
    if node.is_me(&machine) {
        let controlled = control_here(node, &resident, control).await?;
        return Ok((Address { machine, resident }, controlled));
    }
    let call = RemoteCall::Control { resident: resident.clone(), control };
    match node
        .call(&Acting::Here, &(&machine).into(), Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
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
    if node.is_me(&machine) {
        let offers = offers_here(node, &resident).await?;
        return Ok((Address { machine, resident }, offers));
    }
    let call = RemoteCall::Offers { resident: resident.clone() };
    match node
        .call(&Acting::Here, &(&machine).into(), Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(RemoteReply::Offers(offers)) => Ok((Address { machine, resident }, *offers)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than what it offers"))),
        Err(failure) => Err(failure),
    }
}

/// Does `control` to a resident of this machine, through its host. Sessions
/// the World does not hold (the user's own Codex in an app) take nothing.
pub async fn control_here(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    let host = host_of(node, resident)?;
    let gate = node.residents.management(resident)?;
    // Claude commands may wait for user interaction. Stopping their host
    // must remain possible; its bridge reports the interrupted invocation.
    // Answers also pass a queued lifecycle writer so a dialog cannot deadlock it.
    let _reading = if host == "Codex" && !matches!(control, Control::Answer { .. }) {
        Some(crate::gate_waits::shared(node, resident, &gate).await?)
    } else {
        None
    };
    node.residents.active(resident)?;
    match host {
        "Codex" => crate::codex::control::control(node, resident, control).await,
        "Claude" => crate::claude::control::control(node, resident, control).await,
        _ => Err(Failure::new(format!("{host} sessions take nothing but words yet"))),
    }
}

/// What a resident of this machine offers now, as its host says.
pub async fn offers_here(node: &Arc<Node>, resident: &ResidentId) -> Result<Offers, Failure> {
    match host_of(node, resident)? {
        "Codex" => crate::codex::control::offers(node, resident).await,
        "Claude" => crate::claude::control::offers(node, resident).await,
        _ => Ok(Offers::default()),
    }
}

/// The host a resident of this machine is held through, by name.
fn host_of(node: &Node, resident: &ResidentId) -> Result<&'static str, Failure> {
    node.residents.active(resident)?;
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
        None => crate::colleagues::find_for(node, &Acting::Here, resident).await,
    }
}
