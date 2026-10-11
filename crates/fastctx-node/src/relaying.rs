//! A machine its owner made a relay (`world-network.md`, 转接站：只转字节):
//! while the hub's view says it relays at some addresses, this node forwards
//! their ports to the hub, byte for byte, as `fastctx relay` does; when the
//! view says no longer, or the hub's link goes, it stops. The addresses are
//! the ones the others dial; the node listens on every interface at their
//! ports, since a machine behind a router or a cloud's network is reached at
//! an address it does not hold itself.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use fastctx_net::ids::HubId;
use tokio::task::JoinHandle;

use crate::Node;

/// The forwarding this node does, hub by hub.
#[derive(Default)]
pub(crate) struct Relaying {
    running: std::sync::Mutex<BTreeMap<HubId, Forwarding>>,
}

/// The addresses a node was asked to relay at for one hub, and the tasks
/// listening for them; dropped, the tasks stop.
struct Forwarding {
    at: Vec<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Forwarding {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Starts, changes or stops the forwarding for `hub` to match its view.
pub(crate) fn follow(node: &Node, hub: &HubId) {
    let link = node.link_of(hub);
    let wanted: Vec<SocketAddr> = link
        .as_ref()
        .map(|link| {
            let view = link.view();
            view.directory
                .members
                .iter()
                .find(|member| member.name == view.me)
                .map(|member| member.relay_at.clone())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut running = node.relaying.running.lock().expect("never poisoned");
    if running.get(hub).is_some_and(|forwarding| forwarding.at == wanted) {
        return;
    }
    if running.remove(hub).is_some() {
        node.log.line(format_args!("relaying to {hub}: stopped"));
    }
    let Some(link) = link.filter(|_| !wanted.is_empty()) else { return };
    let to = link.hub_addresses();
    let ports: BTreeSet<(bool, u16)> = wanted.iter().map(|address| (address.is_ipv6(), address.port())).collect();
    let mut tasks = Vec::new();
    for (v6, port) in ports {
        let listen = SocketAddr::new(
            if v6 { IpAddr::V6(Ipv6Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::UNSPECIFIED) },
            port,
        );
        let to = to.clone();
        let log = node.log.clone();
        tasks.push(tokio::spawn(async move {
            match fastctx_hub::relay::bind(listen, &to).await {
                Ok(listener) => {
                    log.line(format_args!(
                        "relaying {listen} to the hub at {}",
                        to.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                    ));
                    fastctx_hub::relay::serve(listener, to, quiet).await;
                }
                // Nothing to retry on its own: the port is taken or not this
                // user's to open. The owner sees the machine is not reached
                // through it, and the log says why.
                Err(error) => log.line(format_args!("relaying: {error}")),
            }
        }));
    }
    running.insert(hub.clone(), Forwarding { at: wanted, tasks });
}

/// The forwarding's own account of each connection stays out of the node's
/// log: a relay carries every connection of the machines behind it.
fn quiet(_: std::fmt::Arguments<'_>) {}
