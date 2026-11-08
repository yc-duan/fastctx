//! One coalesced, memory-only skill index per account, invalidated by hub hints.
use crate::Node;
use fastctx_types::access::{HubAccount, HubId};
use fastctx_types::skills::Index;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Default)]
pub(crate) struct Cache(Mutex<HashMap<HubAccount, Arc<Entry>>>);
#[derive(Default)]
struct Entry {
    epoch: AtomicU64,
    body: tokio::sync::Mutex<Option<Held>>,
    /// Why reading the index last failed, while it keeps failing.
    failed: Mutex<Option<String>>,
}

/// The index as last read, what a session is told of it, and its version.
#[derive(Clone)]
pub(super) struct Held {
    epoch: u64,
    pub index: Index,
    pub text: String,
    pub version: String,
}

/// Why an index could not be read, and whether that is news since the last
/// try, worth a line in the log.
pub(super) struct Failed {
    pub error: String,
    pub news: bool,
}
impl Cache {
    pub(crate) fn invalidate(&self, hub: &HubId, account: Option<&fastctx_types::AccountName>) {
        for (scope, entry) in self.0.lock().expect("never poisoned").iter() {
            if scope.hub == *hub && account.is_none_or(|account| *account == scope.account) {
                entry.epoch.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}
pub(super) async fn get(
    node: &Node,
    link: &fastctx_net::link::Link,
    account: &fastctx_types::AccountName,
) -> Result<Held, Failed> {
    let scope = HubAccount { hub: link.membership().id, account: account.clone() };
    let entry = node.skill_index.0.lock().expect("never poisoned").entry(scope).or_default().clone();
    let mut body = entry.body.lock().await;
    loop {
        let epoch = entry.epoch.load(Ordering::Acquire);
        if let Some(held) = &*body
            && held.epoch == epoch
        {
            return Ok(held.clone());
        }
        let (index, version) = match super::index(link, account).await {
            Ok(read) => {
                *entry.failed.lock().expect("never poisoned") = None;
                read
            }
            Err(error) => {
                let news = entry.failed.lock().expect("never poisoned").replace(error.clone()).as_ref() != Some(&error);
                return Err(Failed { error, news });
            }
        };
        let text = fastctx_types::skills::context(&index);
        if entry.epoch.load(Ordering::Acquire) != epoch {
            continue;
        }
        let held = Held { epoch, index, text, version };
        *body = Some(held.clone());
        return Ok(held);
    }
}
