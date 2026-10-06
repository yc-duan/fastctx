//! Calls at a relay. A call between two machines both below it is passed
//! here, checked as the hub checks it, and never goes up; any other call
//! goes up as it came. A call from above is numbered again for this hop, so
//! the numbers this relay gives the machines below it never meet the
//! upstream's; the answer goes back up under the upstream's number.

use fastctx_net::envelope::{self, Envelope};
use fastctx_net::sign::Signed;
use fastctx_net::wire::{CallStatus, Down, Up};
use fastctx_types::MachineName;
use jiff::Timestamp;

use super::{Shared, State};
use crate::Out;

pub(crate) enum Call {
    /// Between two machines below: `caller`'s call `id`, given to `target`.
    Here { caller: MachineName, caller_tag: u64, id: u64, target: MachineName, target_tag: u64 },
    /// From above: the upstream's call `call`, given to `target`.
    Above { call: u64, target: MachineName, target_tag: u64 },
}

impl Call {
    fn target(&self) -> (&MachineName, u64) {
        match self {
            Call::Here { target, target_tag, .. } | Call::Above { target, target_tag, .. } => (target, *target_tag),
        }
    }
}

impl Shared {
    /// Opens an envelope of `name`'s, checking it came from `name`: the hub's
    /// check (`crate::control`), made here for what the hub does not see.
    fn open_envelope(&self, name: &MachineName, message: &Signed) -> Result<Envelope, String> {
        let claimed = envelope::claim(message)?;
        if claimed.from != *name {
            return Err(format!("it says it is from {}, on the connection of {name}", claimed.from));
        }
        if claimed.world != self.world {
            return Err(format!("it is for the World {}", claimed.world));
        }
        let key =
            self.machine.table().get(name).map(|member| member.key).ok_or_else(|| format!("{name} is not a member"))?;
        envelope::open(message, &key)
    }
}

/// A call `name` made: passed to its target when that is below this relay,
/// else up.
pub(super) fn call(shared: &Shared, name: &MachineName, tag: u64, id: u64, message: Signed) {
    let to = envelope::claim(&message).ok().map(|claimed| claimed.to);
    let here = to.as_ref().is_some_and(|to| shared.state().below.get(to).is_some_and(|below| below.here()));
    if !here {
        shared.up_for(name, tag, Up::Call { id, message });
        return;
    }
    let to = match shared.open_envelope(name, &message) {
        Ok(Envelope { to, kind, .. }) if kind.is_call() => to,
        // As at the hub: a caller that sends something unreadable is told
        // nobody by that description is there.
        Ok(_) | Err(_) => {
            shared.tell(name, tag, Down::Status { id, status: CallStatus::NotMember });
            return;
        }
    };
    let number = shared.next();
    let mut state = shared.state();
    let status = match state.below.get(&to).filter(|target| target.here()) {
        Some(target) => {
            let target_tag = target.tag;
            if target.send(&to, Out::Frame(Down::Incoming { call: number, message })) {
                state
                    .calls
                    .insert(number, Call::Here { caller: name.clone(), caller_tag: tag, id, target: to, target_tag });
                CallStatus::Delivered
            } else {
                CallStatus::Offline { last_seen: Some(Timestamp::now()) }
            }
        }
        // It left between the two looks.
        None => CallStatus::Offline { last_seen: Some(Timestamp::now()) },
    };
    if let Some(caller) = state.below.get(name).filter(|caller| caller.tag == tag) {
        caller.send(name, Out::Frame(Down::Status { id, status }));
    }
}

/// A call from above for `name`, numbered again for this hop.
pub(super) fn incoming(shared: &Shared, state: &mut State, name: &MachineName, tag: u64, call: u64, message: Signed) {
    let Some(target) = state.below.get(name).filter(|target| target.tag == tag) else { return };
    let number = shared.next();
    if target.send(name, Out::Frame(Down::Incoming { call: number, message })) {
        state.calls.insert(number, Call::Above { call, target: name.clone(), target_tag: tag });
    }
}

/// `name`'s answer to the call this relay numbered `number`.
pub(super) fn answer(shared: &Shared, name: &MachineName, tag: u64, number: u64, message: Signed) {
    let call = {
        let mut state = shared.state();
        match state.calls.get(&number) {
            Some(call) if call.target() == (name, tag) => state.calls.remove(&number),
            // The caller gave up or left; the answer is dropped, as at the hub.
            _ => None,
        }
    };
    match call {
        Some(Call::Above { call, .. }) => shared.up_for(name, tag, Up::Answer { call, message }),
        Some(Call::Here { caller, caller_tag, id, .. }) => {
            let frame = match shared.open_envelope(name, &message) {
                Ok(Envelope { to, kind, .. }) if kind.is_answer() && to == caller => Down::Answered { id, message },
                Ok(_) | Err(_) => {
                    shared.log(format!("{name} answered call {number} with something that is not its answer"));
                    Down::Status { id, status: CallStatus::Lost }
                }
            };
            shared.tell(&caller, caller_tag, frame);
        }
        None => {}
    }
}

/// `name` no longer waits for its call `id`.
pub(super) fn cancel(shared: &Shared, name: &MachineName, tag: u64, id: u64) {
    let here = {
        let mut state = shared.state();
        let number = state.calls.iter().find_map(|(number, call)| match call {
            Call::Here { caller, caller_tag, id: made, .. } if caller == name && *caller_tag == tag && *made == id => {
                Some(*number)
            }
            _ => None,
        });
        number.and_then(|number| state.calls.remove(&number)).is_some()
    };
    if !here {
        shared.up_for(name, tag, Up::Cancel { id });
    }
}

/// Ends the calls to and from `name`'s attachment `tag`, which left: a
/// caller below waiting on it is told the call is lost. The upstream ends
/// its own when it hears `name` left.
pub(super) fn left(state: &mut State, name: &MachineName, tag: u64) {
    let ended: Vec<u64> = state
        .calls
        .iter()
        .filter(|(_, call)| {
            call.target() == (name, tag)
                || matches!(call, Call::Here { caller, caller_tag, .. } if caller == name && *caller_tag == tag)
        })
        .map(|(number, _)| *number)
        .collect();
    for number in ended {
        if let Some(Call::Here { caller, caller_tag, id, target, .. }) = state.calls.remove(&number)
            && target == *name
            && let Some(waiting) = state.below.get(&caller).filter(|waiting| waiting.tag == caller_tag)
        {
            waiting.send(&caller, Out::Frame(Down::Status { id, status: CallStatus::Lost }));
        }
    }
}
