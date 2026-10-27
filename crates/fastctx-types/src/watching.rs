//! Several conversation subscriptions on one authenticated observation stream.
//! The enclosing call fixes the account; individual subscriptions retain the
//! same conversation and login authorization as an ordinary Watch call.

pub mod local;

use crate::{
    ResidentId,
    ipc::Failure,
    session::{Watched, held::Held},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub resident: ResidentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<crate::composer::sync::Subscription>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub now: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal: bool,
}

impl Subscription {
    pub fn call(&self, watch: u64, multiplex: bool) -> crate::remote::RemoteCall {
        crate::remote::RemoteCall::Watch {
            resident: self.resident.clone(),
            watch,
            composer: self.composer.clone(),
            documents: self.documents.clone(),
            held: self.held.clone(),
            now: self.now,
            goal: self.goal,
            multiplex,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Subscribe { id: u64, subscription: Box<Subscription> },
    Unsubscribe { id: u64 },
    Consumed { id: u64, number: u64 },
    Composer { id: u64, input: Box<crate::composer::sync::Input> },
}

/// Every signed packet binds the stream nonce, subscription and its sequence.
#[derive(Serialize, Deserialize)]
pub struct Packet {
    pub watch: u64,
    pub subscription: u64,
    pub number: u64,
    pub event: Event,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Ready { version: u32, subscriptions: usize },
    Opened,
    Piece { watched: Watched },
    End { failure: Option<Failure> },
}
