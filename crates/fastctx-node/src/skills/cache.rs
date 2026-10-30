//! One coalesced, memory-only skill index per account, invalidated by hub hints.
use crate::Node;
use fastctx_types::access::{HubAccount, HubId};
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
    body: tokio::sync::Mutex<Option<(u64, String, String)>>,
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
) -> Result<(String, String), String> {
    let scope = HubAccount { hub: link.membership().id, account: account.clone() };
    let entry = node.skill_index.0.lock().expect("never poisoned").entry(scope).or_default().clone();
    let mut body = entry.body.lock().await;
    loop {
        let epoch = entry.epoch.load(Ordering::Acquire);
        if let Some((held, text, version)) = &*body
            && *held == epoch
        {
            return Ok((text.clone(), version.clone()));
        }
        let (index, version) = super::index(link, account).await?;
        let mut text = fastctx_types::skills::context(&index);
        if text.len() > 16 * 1024 {
            let mut end = 16 * 1024 - 256;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\nThe skill list continues; `fastctx skill --list` reads all names and descriptions.");
        }
        if entry.epoch.load(Ordering::Acquire) != epoch {
            continue;
        }
        *body = Some((epoch, text.clone(), version.clone()));
        return Ok((text, version));
    }
}
