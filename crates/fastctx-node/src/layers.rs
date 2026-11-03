//! The layer each account chose (`world-layers.md`). The workbench opens in
//! a login now (`crate::logins::workbench`); an account's layer still says
//! where it was open before that was worked out, and the interface shows the
//! workbench's places by it. The user's own sessions are given `run` alone
//! either way (`About::world_managed`).

use std::sync::Arc;

use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;
use fastctx_types::layers::{Chosen, Layer};
use fastctx_types::{AccountName, HubName, ResidentId};

use crate::Node;
use crate::town::caller::Caller;

/// Where a computer in no network keeps its own layer.
fn local_path(node: &Node) -> std::path::PathBuf {
    node.world.path().join("layer.json")
}

fn local(node: &Node) -> Chosen {
    fastctx_net::link::state::read_or(&local_path(node), Chosen::default()).unwrap_or_default()
}

/// The layer chosen, changed to `layer` when given: the account's, or this
/// computer's own while it is in no network.
pub(crate) async fn call(
    node: &Arc<Node>,
    hub: Option<HubName>,
    account: Option<AccountName>,
    layer: Option<Layer>,
) -> Result<Chosen, Failure> {
    if node.links().is_empty() && hub.is_none() && account.is_none() {
        let _writing = node.writing.lock().await;
        let mut chosen = local(node);
        if let Some(layer) = layer.filter(|layer| chosen.layer != Some(*layer)) {
            chosen.layer = Some(layer);
            fastctx_net::link::state::write(&local_path(node), &chosen)
                .map_err(|error| Failure::new(error.to_string()))?;
            changed(node);
        }
        return Ok(chosen);
    }
    let (link, account) = crate::profile::scope(node, &Caller::User, hub.as_ref(), account.as_ref())?;
    match layer {
        Some(layer) => {
            let chosen = link.set_layer(&account, layer).await.map_err(Failure::new)?;
            changed(node);
            Ok(chosen)
        }
        None => link.layer(&account).map_err(Failure::new),
    }
}

/// The layer for whom `acting` works for: the workbench where any of its
/// accounts is on it, or where none of them can be read here, as an
/// account another lets its agent assist. An account that never chose is
/// in Connect, unless this node holds conversations of Fastctx's own for it.
pub(crate) fn of(node: &Node, acting: &Acting) -> Layer {
    let links = node.links();
    if links.is_empty() {
        let chosen = local(node);
        return chosen.effective(chosen.layer.is_none() && holds(node, |_| true));
    }
    let mut read = false;
    for link in &links {
        for account in node.accounts_on(link, acting) {
            if let Ok(chosen) = link.layer(&account) {
                read = true;
                let held =
                    chosen.layer.is_none() && holds(node, |acting| node.accounts_on(link, acting).contains(&account));
                if chosen.effective(held) == Layer::Workbench {
                    return Layer::Workbench;
                }
            }
        }
    }
    if read { Layer::Connect } else { Layer::Workbench }
}

/// The layer for whom `acting` works for, as `of` says it, where every
/// account of it on this machine's networks can be read now: none while
/// one cannot (`crate::logins::workbench::seed`).
pub(crate) fn readable(node: &Node, acting: &Acting) -> Option<Layer> {
    let links = node.links();
    if links.is_empty() {
        let chosen = local(node);
        return Some(chosen.effective(chosen.layer.is_none() && holds(node, |_| true)));
    }
    let mut read = false;
    for link in &links {
        for account in node.accounts_on(link, acting) {
            let chosen = link.layer(&account).ok()?;
            read = true;
            let held =
                chosen.layer.is_none() && holds(node, |acting| node.accounts_on(link, acting).contains(&account));
            if chosen.effective(held) == Layer::Workbench {
                return Some(Layer::Workbench);
            }
        }
    }
    Some(if read { Layer::Connect } else { Layer::Workbench })
}

/// Whether this node holds a conversation of Fastctx's own working for
/// whom `works_for` picks: one started before there were layers, since in
/// Connect none is started.
fn holds(node: &Node, works_for: impl Fn(&Acting) -> bool) -> bool {
    node.residents
        .everyone()
        .any(|held| held.removed.is_none() && held.about.world_managed() && works_for(&held.about.acting))
}

/// Brings a session the user opened on their own into Fastctx
/// (`world-layers.md`, 接进来): from now on the World holds its thread,
/// speaks into it and wakes it as one of its own conversations. Only a
/// Codex thread can be: Claude Code does not tell the World's tools which of
/// its sessions calls them.
pub(crate) fn bring_in(node: &Arc<Node>, resident: &ResidentId) -> Result<(), Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    if about.world_managed() {
        return Ok(());
    }
    if about.host.agent.as_str() != "codex" || about.session.is_none() {
        return Err(Failure::refused(
            "only a Codex session can be brought in: Claude Code does not tell which of its sessions this is",
        ));
    }
    crate::logins::workbench::may_start(node, &about.acting, "codex", about.home.as_deref())?;
    node.residents.set_native_resumed(&node.world, resident).map_err(|error| Failure::new(error.to_string()))?;
    node.shown_changed();
    Ok(())
}

/// The layer changed here: interfaces read it again, and what waited for
/// the workbench is tried at once. On the workbench each login is asked
/// again, its models too.
fn changed(node: &Arc<Node>) {
    node.shown_changed();
    for held in node.residents.everyone().filter(|held| held.about.world_managed()) {
        crate::lifecycle::retry::changed(node, &held.id);
    }
    if of(node, &Acting::Here) == Layer::Workbench {
        let node = node.clone();
        fastctx_platform::task::spawn(async move {
            if let Err(failure) = crate::logins::scan::scan(&node, false).await {
                node.log.line(format_args!("asking the logins again for the workbench: {}", failure.message));
            }
        });
    }
}
