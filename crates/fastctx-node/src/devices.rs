//! What a device may ask of this machine. A device, such as a phone, is a
//! member of the World that carries the user's own words and looks and runs
//! nothing (`fastctx_net::trust::Kind`). What concerns one resident it asks
//! of that resident's machine itself, as a machine does for its user: so
//! the words are known to have been typed on the device, and a watch comes
//! straight from where the conversation lives; a file the user reads, it
//! reads from the machine that has it. What concerns the whole
//! World it hands to any machine's node, which makes the call for it as
//! for a user at that machine, so the World is asked around in one place.
//! A device never runs, writes, copies, asks a colleague or sends a
//! letter: those are a machine's and its residents' own.

use std::sync::Arc;

use fastctx_types::MachineName;
use fastctx_types::calls::Call;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::remote::{RemoteCall, RemoteReply, RemoteResult};

use crate::Node;

/// Whether `from` is a member this machine knows as a device.
pub fn is_device(node: &Node, from: &MachineName) -> bool {
    node.link().and_then(|link| link.members().0.get(from).map(|member| !member.kind.is_machine())).unwrap_or(false)
}

/// Refuses a call `from` may not make: a device asks only for the user's
/// own calls, and only a device has its calls made for it.
pub fn admit(node: &Node, from: &MachineName, call: &RemoteCall) -> Result<(), Failure> {
    let device = is_device(node, from);
    let users_own = matches!(
        call,
        RemoteCall::Talk { .. }
            | RemoteCall::Open { .. }
            | RemoteCall::Stop { .. }
            | RemoteCall::Control { .. }
            | RemoteCall::Offers { .. }
            | RemoteCall::Watch { .. }
            // Reading the user's own files, from the machine that has them,
            // and a file the user attaches to their words, kept in that
            // agent's own place: byte streams, which no relay carries.
            | RemoteCall::ReadFile { .. }
            | RemoteCall::Upload { .. }
            | RemoteCall::Relay { .. }
    );
    if device && !users_own {
        return Err(Failure::new(format!(
            "{from} is a device; a device asks for the user's own calls, and runs, writes and copies nothing"
        )));
    }
    if !device && matches!(call, RemoteCall::Relay { .. }) {
        return Err(Failure::new(format!("{from} is a machine; only a device has its calls made for it")));
    }
    Ok(())
}

/// Makes a device's call that asks of the whole World, as for a user at
/// this machine.
pub async fn relay(node: &Arc<Node>, from: &MachineName, call: serde_json::Value) -> RemoteResult {
    let call: Call = serde_json::from_value(call)
        .map_err(|error| Failure::new(format!("{from} asked an unreadable call: {error}")))?;
    let made = matches!(
        call,
        Call::Who { .. }
            | Call::Since { .. }
            | Call::Look { .. }
            | Call::Agents { .. }
            | Call::Nodes
            | Call::Project(_)
            | Call::Journal(_)
            | Call::Invite { .. }
            | Call::Revoke { .. }
    );
    if !made {
        return Err(Failure::new(
            "a device asks this of the machine it concerns itself, or not at all: only calls that ask of the whole World are made for it",
        ));
    }
    let user = Peer::Command { resident: None, session: None };
    let reply = crate::server::handle(node, &user, call).await?;
    let reply = serde_json::to_value(reply).map_err(|error| Failure::new(format!("writing the answer: {error}")))?;
    Ok(RemoteReply::Relayed { answer: reply })
}
