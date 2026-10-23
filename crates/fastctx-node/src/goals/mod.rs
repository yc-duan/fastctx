//! Durable goals: account authority, local effects, and lifecycle reconciliation.

pub(crate) mod calls;
pub(crate) mod context;
mod intents;
pub(crate) mod lifecycle;
pub(crate) mod operations;
pub(crate) mod records;
mod recovery;
mod wiring;
pub(crate) use wiring::{colleagues, connected};
mod scope;
mod state;
pub(crate) mod watch;
mod watching;
mod words;

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
    pub changed: tokio::sync::Notify,
    recovering: Mutex<HashSet<Address>>,
    lifecycle: Mutex<HashSet<ResidentId>>,
    watching: Mutex<HashSet<HubAccount>>,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            records: Arc::new(records::Unconnected),
            context: Arc::new(context::Unconnected),
            views: Default::default(),
            changed: Default::default(),
            recovering: Default::default(),
            lifecycle: Default::default(),
            watching: Default::default(),
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
}

/// Account-record content is sent only to a viewer of that account, even for a multi-account conversation.
pub(crate) fn visible(
    node: &Node,
    looker: crate::admission::seen::Looker<'_>,
    event: &fastctx_types::session::SessionEvent,
) -> bool {
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
