//! The dedicated local WatchMany connection. Each subscription retains all the
//! routing and account fields of an ordinary Watch call.

use crate::{
    Call, ResidentId,
    access::{HubAccount, Written},
    ipc::Failure,
    session::{Watched, held::Held},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Subscription {
    pub resident: ResidentId,
    pub machine: Option<Written>,
    pub composer: Option<String>,
    pub held: Option<Held>,
    pub now: bool,
    pub goal: Option<HubAccount>,
}

impl Subscription {
    pub fn call(&self) -> Call {
        Call::Watch {
            resident: self.resident.clone(),
            machine: self.machine.clone(),
            composer: self.composer.clone(),
            held: self.held.clone(),
            now: self.now,
            goal: self.goal.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Subscribe { id: u64, subscription: Box<Subscription> },
    Unsubscribe { id: u64 },
    Consumed { id: u64, number: u64 },
}

#[derive(Serialize, Deserialize)]
pub struct Packet {
    pub subscription: u64,
    pub number: u64,
    pub result: Result<Option<Watched>, Failure>,
}
