//! Durable goals: account authority, local effects, and lifecycle reconciliation.

mod archive;
pub(crate) mod calls;
pub(crate) mod colleagues;
pub(crate) mod commands;
pub(crate) mod context;
pub(crate) mod engine;
pub(crate) mod holding;
mod intents;
pub(crate) mod lifecycle;
pub(crate) mod operations;
pub(crate) mod questions;
pub(crate) mod records;
mod recovery;
pub(crate) mod replies;
pub(crate) mod reports;
pub(crate) mod reviewer;
mod wiring;
pub(crate) use wiring::connected;
mod scope;
mod state;
pub(crate) mod waiting;
pub(crate) mod watch;
mod watching;
mod words;
pub(crate) use words::older_goal_waits;

use crate::Node;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::View;
use fastctx_types::layout::Here;
use fastctx_types::{Address, ResidentId};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

pub(crate) struct Services {
    pub records: Arc<dyn records::Records>,
    pub context: Arc<dyn context::Context>,
    pub views: Mutex<BTreeMap<ResidentId, View>>,
    pub changed: tokio::sync::watch::Sender<u64>,
    pub engine: engine::Running,
    pub reviewer: Arc<dyn reviewer::Reviewer>,
    recovering: Mutex<HashSet<Address>>,
    lifecycle: Mutex<HashSet<ResidentId>>,
    watching: Mutex<HashSet<HubAccount>>,
    /// Goals whose reading failed, read again apart until it succeeds
    /// (`watching::refresh_one`).
    retrying: Mutex<HashSet<(HubAccount, String)>>,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            records: Arc::new(records::Unconnected),
            context: Arc::new(context::Unconnected),
            views: Default::default(),
            changed: tokio::sync::watch::channel(0).0,
            engine: Default::default(),
            reviewer: Arc::new(reviewer::Unconnected),
            recovering: Default::default(),
            lifecycle: Default::default(),
            watching: Default::default(),
            retrying: Default::default(),
        }
    }
}

impl Services {
    pub(crate) fn available(&self) -> bool {
        self.records.available() && self.context.available()
    }
}

pub(crate) fn same(node: &Node, first: &Address, second: &Address) -> bool {
    first.resident == second.resident
        && (first.machine == second.machine || node.names().names_of(&first.machine).contains(&second.machine))
}

pub(crate) fn start(node: &Arc<Node>) {
    if !node.goals.available() {
        return;
    }
    recovery::start(node);
    watching::start(node);
    lifecycle::start(node);
    engine::start(node);
}

/// Account-record content is sent only to a viewer of that account, even for a multi-account conversation.
pub(crate) fn visible(
    node: &Node,
    looker: crate::admission::seen::Looker<'_>,
    event: &fastctx_types::session::SessionEvent,
) -> bool {
    if let fastctx_types::session::Happened::Heard { notice: Some(notice), .. } = &event.happened
        && let Some(account) = notice.goal_account()
    {
        return looker.sees_account(node, account);
    }
    let fastctx_types::session::Happened::Changed {
        change: fastctx_types::session::SessionChange::Goal { goal }, ..
    } = &event.happened
    else {
        return true;
    };
    if goal["world"] != true {
        return true;
    }
    serde_json::from_value::<HubAccount>(goal["view"]["account"].clone())
        .is_ok_and(|account| looker.sees_account(node, &account))
}
