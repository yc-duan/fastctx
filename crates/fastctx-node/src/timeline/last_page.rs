//! Admission and revocation for timeline pages kept for display on this node.

use crate::Node;
use fastctx_client::cache::{Cache, LastPageKey};
use fastctx_types::access::{HubId, Written};
use fastctx_types::assets::{Holders, conversation_classes};
use fastctx_types::timeline_pages::{TimelineAsk, TimelineFrom, TimelineTold};
use fastctx_types::{Address, ResidentId};
use std::sync::Arc;

pub(crate) struct Reading {
    identity: LastPageKey,
    generation: u64,
}

fn identity(node: &Node, machine: &Written, resident: &ResidentId) -> Option<LastPageKey> {
    let links = node.locate(machine).ok()?;
    let [link] = links.as_slice() else { return None };
    let view = crate::authorization::current_view(link).ok()?;
    let target = view.member(&machine.name)?;
    let accounts: Vec<_> = view.me()?.logins.iter().cloned().collect();
    let library = &node.assets.as_ref()?.library;
    let hub = link.membership().id.clone();
    // A host-only holding and an ambiguous account have no private display page.
    if node.links().iter().any(|link| {
        let view = link.view();
        view.directory.shares.iter().any(|share| view.is_me(&share.machine))
    }) {
        return None;
    }
    let classes = conversation_classes();
    let mut found = None;
    for scope in library.scopes_ending(&format!("/{resident}")).ok()? {
        if !scope.key.starts_with("c/")
            || scope.hub.as_deref() != Some(hub.as_str())
            || !scope.allowed.as_ref().is_some_and(|allowed| {
                allowed
                    .lines()
                    .any(|name| classes.iter().any(|class| class.holders == Holders::Logged && class.prefix == name))
            })
            || !super::copy::lives_on(library, &scope, &target.name)
        {
            continue;
        }
        for account in &accounts {
            if !fastctx_client::timeline_copy::read_by(&view.directory, &scope, &accounts, Some(account)) {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(LastPageKey {
                hub: hub.clone(),
                address: Address { machine: target.name.clone(), resident: resident.clone() },
                scope: scope.key.clone(),
                holding: scope.holding,
                machine_key: target.key.to_string(),
                account: account.clone(),
                opened: view.directory.account(account)?.opened.as_millisecond(),
            });
        }
    }
    found
}

pub(crate) async fn get(node: &Arc<Node>, machine: Written, resident: ResidentId) -> Option<TimelineTold> {
    let node = node.clone();
    fastctx_platform::task::blocking(move || {
        let access = identity(&node, &machine, &resident)?;
        let page = Cache::under(&node.root).ok()?.last_page(&access).ok()??;
        (identity(&node, &machine, &resident).as_ref() == Some(&access)).then_some(page)
    })
    .await
}

pub(crate) async fn capture(node: &Arc<Node>, machine: &Written, request: &TimelineAsk) -> Option<Reading> {
    use fastctx_types::timeline_pages::{PAGE_BYTES, PAGE_EVENTS};
    if !matches!(
        request,
        TimelineAsk::Page { from: TimelineFrom::Latest, max_events: PAGE_EVENTS, max_bytes: PAGE_BYTES, .. }
    ) {
        return None;
    }
    let (node, machine, resident) = (node.clone(), machine.clone(), request.resident().clone());
    fastctx_platform::task::blocking(move || {
        let generation = Cache::under(&node.root).ok()?.last_page_generation().ok()?;
        Some(Reading { identity: identity(&node, &machine, &resident)?, generation })
    })
    .await
}

pub(crate) fn keep(node: &Arc<Node>, machine: Written, reading: Reading, page: TimelineTold) {
    let node = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        let resident = &reading.identity.address.resident;
        if identity(&node, &machine, resident).as_ref() != Some(&reading.identity) {
            return;
        }
        if let Ok(cache) = Cache::under(&node.root) {
            let _ = cache.keep_last_page(&reading.identity, reading.generation, page);
        }
    });
}

pub(crate) fn discard(node: &Arc<Node>, machine: Written, resident: ResidentId) {
    let node = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        let canonical = node.locate(&machine).ok().and_then(|links| {
            let [link] = links.as_slice() else { return None };
            link.view().member(&machine.name).map(|member| member.name.clone())
        });
        let address = Address { machine: canonical.unwrap_or(machine.name), resident };
        if let Ok(cache) = Cache::under(&node.root)
            && let Err(error) = cache.discard_last_pages(&address)
        {
            node.log.line(format_args!("discarding cached timeline pages: {error}"));
        }
    });
}

pub(crate) fn prune(node: &Node, hub: &HubId) -> std::io::Result<()> {
    let Some(link) = node.link_of(hub) else { return Ok(()) };
    if crate::authorization::current_view(&link).is_err() {
        return Ok(());
    }
    Cache::under(&node.root)?.prune_last_pages(hub, |saved| {
        let machine = Written { name: saved.address.machine.clone(), hub: Some(link.hub_name()) };
        identity(node, &machine, &saved.address.resident).as_ref() == Some(saved)
    })
}
