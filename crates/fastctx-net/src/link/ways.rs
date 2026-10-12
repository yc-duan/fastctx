//! The ways this machine reaches another, as a pause before trying again
//! tells them apart (`crate::again::reopened`): a way that opens is one to
//! try at once (`world-network.md`, 断开不是结束: 另一条路刚通，立刻再试).

use std::sync::atomic::{AtomicU64, Ordering};

use fastctx_types::MachineName;
use jiff::Timestamp;

use super::{Inner, Link};

/// A way to a machine, told apart from every other way to it, earlier or
/// later. Through the hub while this machine's line to the hub is up and
/// the machine is online there, named by both of those connections; else
/// over a direct link, named by it. A direct link while the hub's way is
/// there names nothing new: a phone links only while it has calls in
/// flight, and its links coming and going would otherwise seem to open a
/// way each time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Way {
    Hub { line: u64, since: Option<Timestamp> },
    Direct { link: u64 },
}

static CONNECTIONS: AtomicU64 = AtomicU64::new(1);

/// A number for a connection this process makes, the hub's or a direct
/// one, that no other connection has: a way names its connections by it.
pub(crate) fn connection_number() -> u64 {
    CONNECTIONS.fetch_add(1, Ordering::Relaxed)
}

impl Link {
    /// The way to `machine` there is now, if there is one.
    pub fn way_to(&self, machine: &MachineName) -> Option<Way> {
        let inner = &self.inner;
        let through_hub = inner.connection().and_then(|connection| {
            let view = inner.view();
            let member = view.member(machine)?;
            member.online.then_some(Way::Hub { line: connection.generation, since: member.since })
        });
        through_hub.or_else(|| inner.direct.link(machine).map(|conn| Way::Direct { link: conn.generation }))
    }
}

impl Inner {
    /// The ways to other machines may have changed: the line to the hub
    /// came or went, the hub said anew who is online, or a direct link came
    /// up or went down.
    pub(crate) fn ways_changed(&self) {
        self.node.ways_changed();
    }
}
