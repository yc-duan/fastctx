//! Authorization facts kept in the control frame when a call's body travels as data.

use fastctx_types::access::Purpose;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{Address, AgentName, MachineName};
use serde::{Deserialize, Serialize};

/// Generated from the complete call by the sender and checked against it by the receiver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallDetails {
    pub purpose: Purpose,
    pub login: Option<(AgentName, Option<std::path::PathBuf>)>,
    pub resident: Option<Address>,
}

impl CallDetails {
    pub fn of(from: &MachineName, call: &RemoteCall) -> Self {
        Self {
            purpose: call.purpose(),
            login: crate::message::login_asked(call),
            resident: crate::message::worked_for(from, call),
        }
    }
}
