//! The World's network is a tree rooted at the hub (`world-network.md`, 一个
//! 中枢，一棵树): a machine connects to an upstream, the hub or a relay that
//! carries what is said for the machines below it. A member with management
//! rights says where a machine connects, in a route; a machine that relays
//! says where it can be reached, in its own record. Without a route, a
//! machine connects to the hub.
//!
//! The hub is every machine's last upstream, routed or not: a machine whose
//! relays are all gone still reaches the root, where it learns the route a
//! member with management rights gave it since.

use std::fmt;
use std::net::SocketAddr;

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::link::Membership;
use crate::sign::PublicKey;
use crate::trust::{CertPin, Table, WorldId};

/// Where a relay is reached, said by the relay in its own record: the
/// addresses machines below it dial, and its certificate, which they pin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayOffer {
    pub addresses: Vec<SocketAddr>,
    pub cert: CertPin,
}

/// One upstream a machine may connect to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Upstream {
    Hub,
    Relay { machine: MachineName },
}

impl fmt::Display for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Upstream::Hub => f.write_str("the hub"),
            Upstream::Relay { machine } => write!(f, "the relay {machine}"),
        }
    }
}

/// Where `member` connects: the upstreams it tries, in order, the first its
/// own and the rest the ones it falls back to. Signed by a member with
/// management rights; each member's routes are numbered, and the latest
/// counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub world: WorldId,
    pub member: MachineName,
    pub key: PublicKey,
    pub via: Vec<Upstream>,
    pub version: u64,
    pub by: MachineName,
    pub at: Timestamp,
}

impl Route {
    /// The upstreams of a member nobody routed: the hub.
    pub fn direct() -> Vec<Upstream> {
        vec![Upstream::Hub]
    }
}

/// An upstream as a machine dials it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub upstream: Upstream,
    pub addresses: Vec<SocketAddr>,
    pub cert: CertPin,
}

impl Endpoint {
    pub fn hub(membership: &Membership) -> Self {
        Self { upstream: Upstream::Hub, addresses: membership.hub.clone(), cert: membership.cert }
    }
}

/// The upstreams `name` dials, in the order it tries them: its route, each
/// relay where its record says it is reached, then the hub when the route
/// leaves it out. A relay that offers nothing now is passed over, and so is
/// `name` itself: a relay's own upstream is never itself.
pub fn endpoints(table: &Table, membership: &Membership, name: &MachineName) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();
    for upstream in table.upstreams(name) {
        let endpoint = match &upstream {
            Upstream::Hub => Endpoint::hub(membership),
            // A relay whose own route leads through `name` is below it: a
            // route that makes a loop is not followed into it.
            Upstream::Relay { machine } if machine == name || table.carries(name, machine) => continue,
            Upstream::Relay { machine } => match table.get(machine).and_then(|member| member.relay.as_ref()) {
                Some(offer) => Endpoint { upstream: upstream.clone(), addresses: offer.addresses.clone(), cert: offer.cert },
                None => continue,
            },
        };
        if !endpoints.contains(&endpoint) {
            endpoints.push(endpoint);
        }
    }
    if !endpoints.iter().any(|endpoint| endpoint.upstream == Upstream::Hub) {
        endpoints.push(Endpoint::hub(membership));
    }
    endpoints
}

/// Whether a machine on `on`, which it got onto after trying `tried`, should
/// connect again now that its upstreams are `now`: `on` is not one of them
/// any more, or one it never tried comes before it. One it tried and could
/// not reach is asked again in its own time, not here.
pub fn moved(now: &[Endpoint], on: &Endpoint, tried: &[Endpoint]) -> bool {
    match now.iter().position(|endpoint| endpoint == on) {
        None => true,
        Some(at) => now[..at].iter().any(|endpoint| !tried.contains(endpoint)),
    }
}

/// Where `name` hangs in the tree, as the members stand, for showing.
pub fn place(table: &Table, name: &MachineName) -> Option<fastctx_types::world::Place> {
    let member = table.get(name)?;
    let via = table
        .upstreams(name)
        .into_iter()
        .map(|upstream| match upstream {
            Upstream::Hub => fastctx_types::world::Upstream::Hub,
            Upstream::Relay { machine } => fastctx_types::world::Upstream::Relay { machine },
        })
        .collect();
    let relays_at = member
        .relay
        .iter()
        .flat_map(|offer| offer.addresses.iter().map(ToString::to_string))
        .collect();
    Some(fastctx_types::world::Place { via, relays_at })
}
