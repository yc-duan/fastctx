//! What the user does to a conversation besides speaking into it, and what
//! a conversation offers (`fastctx_types::session::control`). The call goes
//! to the machine the resident lives on, as a stop does, and that machine
//! asks the resident's host: a Codex thread of its own app-server through
//! the app-server, a Claude session through the World's mod.

use fastctx_types::access::Written;
use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::{Address, ResidentId};

use crate::Node;

pub async fn control(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    control: Control,
) -> Result<(Address, Controlled), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = machine_of(node, &resident, machine).await?;
    if local || node.is_here(&machine) {
        let controlled = control_here(node, &resident, control).await?;
        return Ok((Address { machine: machine.name, resident }, controlled));
    }
    if matches!(&control, Control::Command { name, .. } if name == "btw") {
        let (_, offered) = offers(node, resident.clone(), Some(machine.clone())).await?;
        if !offered.control_reply_envelope {
            return Err(Failure::not_run(
                "The conversation's Fastctx is too old to return /btw; update it before running this command.",
            ));
        }
    }
    let call = RemoteCall::Control { resident: resident.clone(), control, reply_envelope: true };
    match node
        .call(&Acting::Here, &machine, Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(RemoteReply::Controlled(controlled) | RemoteReply::ControlResult { controlled }) => {
            Ok((Address { machine: machine.name, resident }, controlled))
        }
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than what was done"))),
        Err(failure) => Err(failure),
    }
}

pub async fn offers(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<(Address, Offers), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = machine_of(node, &resident, machine).await?;
    if local || node.is_here(&machine) {
        let offers = offers_here(node, &resident).await?;
        return Ok((Address { machine: machine.name, resident }, offers));
    }
    let call = RemoteCall::Offers { resident: resident.clone() };
    match node
        .call(&Acting::Here, &machine, Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(RemoteReply::Offers(offers)) => Ok((Address { machine: machine.name, resident }, *offers)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than what it offers"))),
        Err(failure) => Err(failure),
    }
}

/// Does `control` to a resident of this machine, through its host. Sessions
/// the World does not hold (the user's own Codex in an app) take nothing.
pub async fn control_here(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    node.ready_for(resident).await;
    let host = host_of(node, resident)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    node.settled_for(&about.acting).await;
    // Each control is told in its conversation as it begins and ends
    // (`world-gui.md`, 对话); one refused here is told as it is refused.
    let told = crate::control_queue::told(&control);
    if let Err(failure) = allowed(node, resident, &about, host, &control) {
        if !told {
            return Err(failure);
        }
        let row = crate::control_queue::begin(node, resident, &control)?;
        return crate::control_queue::end(node, resident, row, Err(failure)).await;
    }
    if let Control::CancelControl { id } = &control {
        return crate::control_queue::cancel(node, resident, id).await;
    }
    if matches!(control, Control::Answer { .. }) {
        return execute(node, resident, control).await;
    }
    let gate = crate::control_queue::gate(node, resident);
    let _action = crate::gate_waits::action(node, resident, &gate).await?;
    node.residents.active(resident)?;
    crate::control_queue::drain(node, resident).await?;
    if let Some(queued) = crate::control_queue::enqueue_if_busy(node, resident, &control).await? {
        return Ok(queued);
    }
    if !told {
        // What is only read waits for no long control: its host answers
        // nothing else until that is done.
        if node.long_controls.of(resident) {
            return Err(Failure::not_run(crate::control_long::WAITING));
        }
        return execute(node, resident, control).await;
    }
    let row = crate::control_queue::begin(node, resident, &control)?;
    if let Err(error) = crate::control_queue::prepare(node, resident, &row).await {
        return crate::control_queue::end(node, resident, row, Err(error)).await;
    }
    if crate::control_long::long(node, resident, &control) {
        return Ok(crate::control_long::start(node, resident, row));
    }
    let result = execute(node, resident, control).await;
    crate::control_queue::end(node, resident, row, result).await
}

/// Whether `control` may be done here: an assisting agent's conversation
/// takes its host's own commands only where they are safe for the owner.
fn allowed(
    node: &Arc<Node>,
    resident: &ResidentId,
    about: &fastctx_types::residents::About,
    host: &str,
    control: &Control,
) -> Result<(), Failure> {
    if crate::assists::assisting_known(node, &about.acting)? {
        crate::assists::host::require(node, resident)?;
        if let Control::Command { name, path, .. } = control {
            let allowed = match host {
                "Claude" => fastctx_hosts::claude::assisted::command_allowed(name),
                "Codex" => fastctx_hosts::codex::assisted::command_allowed(name),
                _ => false,
            };
            if path.is_some() || !allowed {
                return Err(Failure::not_run("This command is unavailable in an assisted conversation."));
            }
        }
    }
    Ok(())
}

pub(crate) async fn execute(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    let host = host_of(node, resident)?;
    let gate = node.residents.management(resident)?;
    // Claude commands may wait for user interaction. Stopping their host
    // must remain possible; its bridge reports the interrupted invocation.
    // Answers also pass a queued lifecycle writer so a dialog cannot deadlock it.
    let forking = matches!(control, Control::Fork { .. })
        || matches!(&control, Control::Command { name, path: None, .. } if name == "fork");
    let _reading = if host == "Codex" && !matches!(control, Control::Answer { .. }) && !forking {
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
    let mut offers = match host_of(node, resident)? {
        "Codex" => crate::codex::control::offers(node, resident).await?,
        "Claude" => crate::claude::control::offers(node, resident).await?,
        _ => Offers::default(),
    };
    offers.pending_controls = crate::control_queue::pending(node, resident)?;
    offers.control_queue = true;
    offers.controls_told = true;
    offers.control_reply_envelope = true;
    Ok(offers)
}

/// The host a resident of this machine is held through, by name.
fn host_of(node: &Node, resident: &ResidentId) -> Result<&'static str, Failure> {
    node.residents.active(resident)?;
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    match (about.host.agent.as_str(), about.world_managed(), &about.session) {
        ("codex", true, Some(_)) => Ok("Codex"),
        ("claude", _, _) => Ok("Claude"),
        _ => Err(Failure::new(format!("{resident} is a session the World does not hold; do this where it is open"))),
    }
}

async fn machine_of(
    node: &Arc<Node>,
    resident: &ResidentId,
    machine: Option<Written>,
) -> Result<fastctx_types::access::Written, Failure> {
    match machine {
        Some(machine) => Ok(machine),
        None if node.residents.about(resident).is_some() => Ok(node.machine().into()),
        None => crate::colleagues::find_for(node, &Acting::Here, resident).await,
    }
}
