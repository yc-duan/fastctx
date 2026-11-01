//! Reverse account indexes and coalesced presence fanout. The work for one
//! member visits only potential readers, then applies the unchanged policy.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_net::directory_index::Indexed;
use fastctx_types::{AccountName, MachineName};

#[derive(Default)]
pub(crate) struct Audience {
    acting: HashMap<AccountName, Vec<MachineName>>,
    using: HashMap<AccountName, Vec<MachineName>>,
    whole: Vec<MachineName>,
}

impl Audience {
    pub(crate) fn new(index: &Indexed<'_>) -> Self {
        let mut audience = Self::default();
        for member in &index.all.members {
            if index
                .acts_for(&member.name)
                .iter()
                .any(|account| index.logged_in(&member.name, account) && index.is_super(account))
            {
                audience.whole.push(member.name.clone());
            }
            for account in index.acts_for(&member.name) {
                audience.acting.entry(account).or_default().push(member.name.clone());
            }
            for account in crate::views::users_in(index, &member.name) {
                audience.using.entry(account).or_default().push(member.name.clone());
            }
        }
        audience
    }

    pub(crate) fn readers(&self, index: &Indexed<'_>, machine: &MachineName) -> BTreeSet<MachineName> {
        let users = crate::views::users_in(index, machine);
        let actors = index.acts_for(machine);
        users
            .iter()
            .flat_map(|account| self.acting.get(account).into_iter().flatten())
            .chain(actors.iter().flat_map(|account| self.using.get(account).into_iter().flatten()))
            .chain(&self.whole)
            .cloned()
            .collect()
    }
}

pub(crate) async fn run(hub: Weak<crate::Hub>, changed: Arc<tokio::sync::Notify>) {
    loop {
        changed.notified().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(hub) = hub.upgrade() else { return };
        let _ = fastctx_platform::task::spawn_blocking(move || {
            let names = std::mem::take(&mut *hub.pending_presence.lock().expect("presence changes"));
            for (name, rev) in names {
                hub.publish_presence(&name, rev);
            }
            hub.machine_log_changed.notify_one();
            hub.assists_changed.notify_one();
            crate::moving::follow::settle(&hub);
            crate::forget::prune(&hub);
        })
        .await;
    }
}
