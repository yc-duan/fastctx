//! The frozen protocol-5 subset used by already running tool adapters and
//! job supervisors. A GUI is never admitted through this compatibility path.
//! It can discover the mismatch and renew through the maintenance envelope.

use fastctx_types::ipc::Peer;
use fastctx_types::residents::Notice;
use fastctx_types::{Call, Reply};

pub(crate) fn peer(peer: &Peer) -> bool {
    matches!(
        peer,
        Peer::Session { .. }
            | Peer::Supervisor { .. }
            | Peer::Command { resident: Some(_), .. }
            | Peer::Command { session: Some(_), .. }
    )
}

pub(crate) fn call(peer: &Peer, call: &Call) -> bool {
    if let Peer::Supervisor { job: owned } = peer {
        return matches!(call, Call::JobChanged { job, .. } if owned == job);
    }
    matches!(
        call,
        Call::Arrive { .. }
            | Call::Run { .. }
            | Call::Write { .. }
            | Call::Copy { .. }
            | Call::Ask { .. }
            | Call::Message { .. }
            | Call::Collect { .. }
            | Call::Taken { .. }
            | Call::Who { .. }
            | Call::Look { .. }
            | Call::Saw { .. }
            | Call::Project(_)
            | Call::Journal(_)
            | Call::Nodes
    )
}

pub(crate) fn reply(reply: &mut Reply) {
    if let Reply::Notices { notices, .. } = reply {
        for delivery in notices {
            if let Notice::Attached { message, text } = &delivery.notice {
                delivery.notice = Notice::Words { from: message.speaker(), text: message.render(text) };
            }
        }
    }
}
