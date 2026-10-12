//! This computer as the new place of the hub (`world-network.md`, 把中枢搬到别处).
//!
//! A move of the hub to this machine shows in its view. The node takes the
//! hub's identity, makes the new hub's directory beside this World and says
//! so; the hub's system service is then installed with administrator rights
//! on this machine (`fastctx hub arrive`, which tells the node when it is
//! done), and the old hub reaches the new one and hands over. A move called
//! off before the service was installed leaves nothing behind here: the
//! node deletes the directory it made. One called off later is the new
//! hub's to give up, which the old hub tells it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use fastctx_net::link::Link;
use fastctx_net::moving::ArrivalReport;
use fastctx_types::access::HubId;
use fastctx_types::hub_move::{InterfaceAddress, MoveStage};
use fastctx_types::ipc::Failure;

use crate::Node;

/// The moves this node is preparing for now, so a view that changes again
/// meanwhile does not start another.
static PREPARING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The moves called off this node told the hub nothing is left of here.
static GONE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Looks at the view of the hub `hub` for a move to this machine.
pub fn check(node: &Arc<Node>, hub: &HubId) {
    let Some(link) = node.link_of(hub) else { return };
    let view = link.view();
    for moving in view.hub_moves.iter().filter(|moving| moving.to == view.me) {
        match moving.stage {
            // A failure waits for the super administrator to try again.
            MoveStage::Preparing if moving.error.is_none() => prepare(node, &link, &moving.id),
            MoveStage::Cancelled => called_off(node, &link, &moving.id),
            _ => {}
        }
    }
}

fn prepare(node: &Arc<Node>, link: &Arc<Link>, id: &str) {
    if !PREPARING.lock().expect("never poisoned").insert(id.to_string()) {
        return;
    }
    let (node, link, id) = (node.clone(), link.clone(), id.to_string());
    tokio::spawn(async move {
        let dir = fastctx_hub::default_dir(&node.root);
        let made = async {
            let identity = link.hub_identity(&id).await?;
            let me = link.membership().name.clone();
            let place = dir.clone();
            let made_id = id.clone();
            tokio::task::spawn_blocking(move || fastctx_hub::prepare_arrival(&place, &made_id, &me, &identity))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("making the new hub's directory {}: {error}", dir.display()))
        };
        let report = match made.await {
            Ok(()) => {
                node.log.line(format_args!(
                    "the hub {} moves here: its directory is {}; its service is to be installed with `fastctx hub arrive`",
                    link.hub_name(),
                    dir.display()
                ));
                ArrivalReport::Prepared { dir: crate::world::native(&dir), os: std::env::consts::OS.to_string() }
            }
            Err(reason) => {
                node.log.line(format_args!("the hub {} cannot move here: {reason}", link.hub_name()));
                ArrivalReport::Failed { reason }
            }
        };
        if let Err(error) = link.hub_arrival(&id, report).await {
            node.log.line(format_args!("telling the hub how the move {id} stands here: {error}"));
        }
        PREPARING.lock().expect("never poisoned").remove(&id);
    });
}

/// Deletes the directory made for the move `id`, called off, unless the
/// hub's service runs it, and tells the hub once nothing of the move is
/// here any more. A service running it gives it up itself when the old hub
/// tells it, and leaves nothing of the move here either.
fn called_off(node: &Arc<Node>, link: &Arc<Link>, id: &str) {
    let dir = fastctx_hub::default_dir(&node.root);
    if holds(&dir, id) {
        let service = std::env::current_exe().map(|executable| {
            fastctx_install::service::Service::new(fastctx_install::service::Kind::Hub, executable, &dir)
        });
        match service.map_err(anyhow::Error::from).and_then(|service| service.status()) {
            Ok(status) if status.installed => {
                return node.log.line(format_args!(
                    "the move {id} was called off; the hub's service here gives it up when the old hub tells it"
                ));
            }
            Ok(_) => match std::fs::remove_dir_all(&dir) {
                Ok(()) => node.log.line(format_args!("the move {id} was called off; {} is deleted", dir.display())),
                Err(error) => {
                    node.log.line(format_args!("the move {id} was called off; deleting {}: {error}", dir.display()))
                }
            },
            Err(error) => {
                return node.log.line(format_args!(
                    "the move {id} was called off; whether a service runs {} is not known ({error}), so it stays",
                    dir.display()
                ));
            }
        }
        if holds(&dir, id) {
            return;
        }
    }
    if !GONE.lock().expect("never poisoned").insert(id.to_string()) {
        return;
    }
    let (node, link, id) = (node.clone(), link.clone(), id.to_string());
    tokio::spawn(async move {
        if let Err(error) = link.hub_arrival(&id, ArrivalReport::Gone).await {
            node.log.line(format_args!("telling the hub nothing of the move {id} is here: {error}"));
            GONE.lock().expect("never poisoned").remove(&id);
        }
    });
}

/// Whether `dir` holds the new hub of the move `id`.
fn holds(dir: &std::path::Path, id: &str) -> bool {
    matches!(fastctx_hub::arriving(dir), Ok(Some((_, arriving))) if arriving == id)
}

/// The hub's service for the move to this machine is installed here: the
/// hub is told, and the old hub reaches the new one now.
pub async fn installed(node: &Arc<Node>, hub: &HubId) -> Result<(), Failure> {
    let link = node.link_of(hub).ok_or_else(|| Failure::new(format!("this computer is not on the hub {hub}")))?;
    link.refresh().await.map_err(Failure::new)?;
    let view = link.view();
    let moving = view
        .hub_moves
        .iter()
        .find(|moving| moving.to == view.me && moving.stage < MoveStage::Following)
        .ok_or_else(|| Failure::new(format!("the hub {} is not moving to this computer", view.hub_name)))?;
    link.hub_arrival(&moving.id, ArrivalReport::Installed).await.map_err(Failure::new)
}

/// This machine's network addresses, physical ones with a way out first.
pub fn interfaces() -> Result<Vec<InterfaceAddress>, Failure> {
    let mut interfaces = fastctx_net::route::interfaces().map_err(Failure::new)?;
    interfaces.sort_by_key(|interface| (!interface.physical, !interface.gateway, interface.metric));
    Ok(interfaces
        .iter()
        .flat_map(|interface| {
            interface.ipv4.iter().filter(|address| !address.is_loopback() && !address.is_link_local()).map(|address| {
                InterfaceAddress {
                    interface: interface.name.clone(),
                    address: (*address).into(),
                    physical: interface.physical,
                }
            })
        })
        .collect())
}
