//! Effects supplied by the resident node; no ambient machine or account access.
use fastctx_agent::FutureResult;
use fastctx_types::{MachineName, sentinel::Definition};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadResult {
    pub text: String,
    pub exit_code: Option<i32>,
    pub full_path: Option<String>,
}
/// A machine a sentinel may look at: one its account can work on, as the
/// home sees it now, like the machines a World agent is given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub name: MachineName,
    pub online: bool,
}
/// Implementations enforce current account authority and a read-only offline sandbox.
pub trait Look: Send + Sync {
    fn look<'a>(
        &'a self,
        definition: &'a Definition,
        machine: &'a MachineName,
        command: &'a str,
    ) -> FutureResult<'a, ReadResult>;
}
pub trait Web: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str) -> FutureResult<'a, ReadResult>;
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult>;
}
/// Only the sentinel's home receives the secret. It is never part of settings replies.
pub trait Credentials: Send + Sync {
    fn status<'a>(
        &'a self,
        scope: &'a fastctx_types::access::HubAccount,
        home: &'a MachineName,
    ) -> FutureResult<'a, fastctx_types::sentinel_settings::KeyStatus>;
    fn model_key<'a>(&'a self, scope: &'a fastctx_types::access::HubAccount, definition: &'a Definition) -> FutureResult<'a, String>;
    fn search_key<'a>(&'a self, scope: &'a fastctx_types::access::HubAccount, definition: &'a Definition) -> FutureResult<'a, String>;
}

/// A node's resident worker. It verifies the authoritative account record and
/// assignment before beginning, and verifies scope against local state on reads.
pub trait ResidentHost: Send + Sync {
    fn call<'a>(
        &'a self,
        scope: &'a fastctx_types::access::HubAccount,
        request: &'a fastctx_types::sentinel_control::HomeRequest,
    ) -> FutureResult<'a, fastctx_types::sentinel_control::HomeReply>;
}

/// Durable timer adapter supplied by the node's scheduler. Reusing an ID replaces
/// its time/action. A firing is delivered at least once with its original due time.
pub trait Timers: Send + Sync {
    /// Subscribe before reconciliation; unacknowledged firings are retried by the scheduler.
    fn subscribe(&self) -> FutureResult<'_, tokio::sync::broadcast::Receiver<Fired>>;
    /// Acknowledge exactly this firing, never a later replacement of its timer ID.
    fn acknowledge<'a>(&'a self, fired: &'a Fired) -> FutureResult<'a, ()>;
    fn set<'a>(&'a self, timer: &'a Timer) -> FutureResult<'a, ()>;
    fn cancel<'a>(&'a self, id: &'a str) -> FutureResult<'a, ()>;
    fn list<'a>(&'a self, owner: &'a str) -> FutureResult<'a, Vec<Timer>>;
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub owner: String,
    pub at: i64,
    pub action: TimerAction,
}
pub use fastctx_types::sentinel::{ScheduledWake, TimerAction};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fired {
    pub timer: Timer,
    pub scheduled_at: i64,
    pub late_ms: u64,
}
