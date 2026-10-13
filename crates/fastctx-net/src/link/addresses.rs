//! Where this machine reaches its hub (`world-network.md`, 把中枢搬到别处).
//!
//! A machine reaches the hub at the addresses it logged in with, until the
//! hub moves: a hub that was moved to where it runs gives every member, in
//! its view, the addresses to reach it at now, the old place's among them
//! while that still forwards. The member keeps them in its membership file,
//! so it dials them after a restart too, and connects again by them when
//! the address it is connected at is no longer among them.

use std::net::SocketAddr;

use super::Inner;
use crate::dial::Upstream;
use crate::directory::View;

impl Inner {
    /// Where the hub is reached now, in the order to try.
    pub(crate) fn hub_addresses(&self) -> Vec<SocketAddr> {
        self.addresses.lock().expect("never poisoned").clone()
    }

    /// Takes the addresses `view` gives, when it gives some that differ
    /// from those held. A connection already at the first of them stays: it
    /// is marked as on the new list, so the change of list alone does not
    /// end it. One at another address, the old place's among them, ends,
    /// and the next one tries the first address first.
    pub(super) fn follow_addresses(&self, view: &View) {
        if view.hub_addresses.is_empty() {
            return;
        }
        let before = {
            let mut held = self.addresses.lock().expect("never poisoned");
            if *held == view.hub_addresses {
                return;
            }
            std::mem::replace(&mut *held, view.hub_addresses.clone())
        };
        match self.keep_membership(|kept| kept.addresses = view.hub_addresses.clone()) {
            Ok(()) => self.node.log(format!(
                "the hub is reached at {} now, no longer at {}",
                list(&view.hub_addresses),
                list(&before)
            )),
            // The new list holds while the node runs; after a restart the
            // old one is dialed, and the hub gives the new one again.
            Err(error) => self.node.log(format!(
                "the hub is reached at {} now; recording that failed: {error}",
                list(&view.hub_addresses)
            )),
        }
        let mut connection = self.connection.lock().expect("never poisoned");
        if let Some(connection) = connection.as_mut()
            && matches!(connection.upstream, Upstream::Hub(_))
            && view.hub_addresses.first() == Some(&connection.address)
        {
            connection.upstream = Upstream::Hub(view.hub_addresses.clone());
            for tried in connection.tried.iter_mut().filter(|tried| matches!(tried, Upstream::Hub(_))) {
                *tried = Upstream::Hub(view.hub_addresses.clone());
            }
        }
    }
}

impl super::Link {
    /// Reaches the hub at `addresses` from now on, as given by logging in
    /// again: kept in the membership file, and the link connects anew.
    pub fn reach_hub_at(&self, addresses: Vec<SocketAddr>) {
        let inner = &self.inner;
        let before = std::mem::replace(&mut *inner.addresses.lock().expect("never poisoned"), addresses.clone());
        match inner.keep_membership(|kept| kept.addresses = addresses.clone()) {
            Ok(()) => inner.node.log(format!(
                "the hub is reached at {} now, no longer at {}",
                list(&addresses),
                list(&before)
            )),
            Err(error) => inner
                .node
                .log(format!("the hub is reached at {} now; recording that failed: {error}", list(&addresses))),
        }
        inner.rerouted.notify_waiters();
        inner.reconnect.notify_one();
    }
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
