//! A machine's direct links with others on one hub (`world-network.md`,
//! 直连).
//!
//! The roster the hub gives (`View`) says which machines the network's
//! administrator allowed direct connections to, and at what addresses. A
//! computer keeps a direct link with every such machine in its roster,
//! dialing again whenever it drops; when both ends allow direct connections,
//! the one whose name sorts first dials. A phone links only while it has a
//! call to that machine in flight, so it does not hold connections open for
//! nothing. Calls, their data connections and subscriptions between two
//! linked machines go over the link, unseen by the hub; when there is no
//! link, they go through the hub as before.
//!
//! A machine that allows direct connections listens at the ports of its
//! addresses, from the roster it last had, whether or not it reaches the
//! hub now; it checks who connects against that roster, and forwards the
//! hub's bytes for machines whose own line to the hub is down.

mod data;
mod listen;
mod peer;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use fastctx_types::access::Purpose;
use fastctx_types::accounts::LinkSeen;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use tokio::task::JoinHandle;

pub use data::DataWay;
pub(crate) use peer::{Answering, PeerConn};

use super::Inner;
use crate::directory::View;

/// What a machine noted of a call it made directly, for the hub, which
/// notes it of the calls it passes on (`wire::Question::Worked`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Worked {
    pub to: MachineName,
    pub account: AccountName,
    pub purpose: fastctx_types::access::Purpose,
    pub resident: fastctx_types::Address,
}

#[derive(Default)]
pub(crate) struct Direct {
    /// The listener, while this machine allows direct connections.
    listening: Mutex<Option<Listening>>,
    /// The live links, by the machine at the other end.
    links: Mutex<HashMap<MachineName, Ends>>,
    /// The tasks keeping this machine's links with others, by machine.
    keepers: Mutex<HashMap<MachineName, Keeper>>,
    /// How each link stands, for the interfaces and the hub.
    seen: Mutex<BTreeMap<MachineName, LinkSeen>>,
    /// Calls other machines made over direct links that this one answers.
    answering: Mutex<HashMap<u64, Answering>>,
    /// The data connections other machines opened that no call here took
    /// yet, and the calls here waiting for one, in one lock (`data`).
    pairing: Mutex<data::Pairing>,
    /// How many calls a phone has in flight to each machine.
    demand: Mutex<HashMap<MachineName, usize>>,
    /// What this machine noted of its direct calls, not yet told to the hub.
    worked: Mutex<Vec<Worked>>,
    /// The addresses of the hub's check this machine last said it listens
    /// for.
    reported: Mutex<Vec<SocketAddr>>,
    /// The ports this machine's firewall is to let in.
    firewall: Arc<Firewall>,
}

#[derive(Default)]
struct Firewall {
    /// What was last asked.
    wanted: Mutex<BTreeSet<u16>>,
    /// Held while a change is made.
    applying: Mutex<()>,
}

/// One machine's links: the one this machine dialed, and the one it took.
#[derive(Default)]
struct Ends {
    dialed: Option<Arc<PeerConn>>,
    taken: Option<Arc<PeerConn>>,
}

/// The ports this machine listens at for direct connections, each with the
/// task accepting there or why it could not listen there.
#[derive(Default)]
struct Listening {
    /// What the listener shows, made when it first listens.
    tls: Option<tokio_rustls::TlsAcceptor>,
    ports: BTreeMap<(bool, u16), Result<JoinHandle<()>, String>>,
}

impl Listening {
    /// Why it could not listen at some port, if it could not.
    fn failed(&self) -> Option<String> {
        let failures: Vec<&String> = self.ports.values().filter_map(|port| port.as_ref().err()).collect();
        (!failures.is_empty()).then(|| failures.into_iter().cloned().collect::<Vec<_>>().join("; "))
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        for task in self.ports.values().flatten() {
            task.abort();
        }
    }
}

struct Keeper {
    at: Vec<SocketAddr>,
    task: JoinHandle<()>,
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The ports to listen at for `at`: an IPv4 and an IPv6 listener on every
/// interface for each port, since a machine behind a router or a cloud's
/// network is reached at an address it does not hold itself.
fn ports(at: &[SocketAddr]) -> BTreeSet<(bool, u16)> {
    at.iter().map(|address| (address.is_ipv6(), address.port())).collect()
}

impl Direct {
    /// Stops listening and linking: the link to the hub stopped.
    pub(crate) fn stop(&self) {
        self.listening.lock().expect("never poisoned").take();
        self.keepers.lock().expect("never poisoned").clear();
        let ends: Vec<Ends> = self.links.lock().expect("never poisoned").drain().map(|(_, ends)| ends).collect();
        for ends in ends {
            for conn in [ends.dialed, ends.taken].into_iter().flatten() {
                conn.close("the link to the hub was stopped".to_string());
            }
        }
    }

    /// The live link to `peer`: the one this machine dialed, else the one it
    /// took.
    pub(crate) fn link(&self, peer: &MachineName) -> Option<Arc<PeerConn>> {
        let links = self.links.lock().expect("never poisoned");
        let ends = links.get(peer)?;
        ends.dialed.clone().or_else(|| ends.taken.clone())
    }

    /// This machine's links as the interfaces show them.
    pub(crate) fn seen(&self) -> Vec<LinkSeen> {
        let seen = self.seen.lock().expect("never poisoned").clone();
        seen.into_values()
            .map(|mut link| {
                if link.up {
                    link.ms = self.link(&link.peer).and_then(|conn| conn.ms());
                }
                link
            })
            .collect()
    }

    fn mark(&self, peer: &MachineName, up: bool, why: Option<String>) {
        let mut seen = self.seen.lock().expect("never poisoned");
        let previous = seen.get(peer).map(|link| link.up);
        let link = LinkSeen { peer: peer.clone(), up, ms: None, since: Timestamp::now(), why };
        if previous == Some(up) {
            // A failure while already down only says why anew.
            if let Some(existing) = seen.get_mut(peer) {
                existing.why = link.why;
            }
            return;
        }
        seen.insert(peer.clone(), link);
    }

    fn forget(&self, peer: &MachineName) {
        self.seen.lock().expect("never poisoned").remove(peer);
    }

    /// Notes a call made directly, to tell the hub.
    pub(crate) fn note_worked(&self, worked: Worked) {
        let mut noted = self.worked.lock().expect("never poisoned");
        if !noted.contains(&worked) {
            noted.push(worked);
        }
    }

    /// What was noted of direct calls and not yet told the hub.
    pub(crate) fn take_worked(&self) -> Vec<Worked> {
        std::mem::take(&mut *self.worked.lock().expect("never poisoned"))
    }

    /// Whether this machine listens now, and why not when it should.
    pub(crate) fn listening(&self) -> Option<Result<(), String>> {
        let listening = self.listening.lock().expect("never poisoned");
        listening.as_ref().map(|listening| match listening.failed() {
            Some(why) => Err(why),
            None => Ok(()),
        })
    }
}

impl Inner {
    /// Brings listening, linking and the calls being answered over links in
    /// line with the roster: run at the start and whenever the view changes.
    pub(crate) fn follow_view(self: &Arc<Self>) {
        let view = self.view();
        self.follow_listening(&view);
        self.follow_keepers(&view);
        self.follow_links(&view);
    }

    fn follow_listening(self: &Arc<Self>, view: &View) {
        // Where it takes direct connections, and where it relays its
        // owner's other machines to the hub.
        let mut at: Vec<SocketAddr> =
            view.me().map(|me| me.direct.iter().chain(&me.relay).copied().collect()).unwrap_or_default();
        at.extend(view.trial.iter().copied());
        let wanted = ports(&at);
        let started = {
            let mut listening = self.direct.listening.lock().expect("never poisoned");
            if wanted.is_empty() {
                if listening.take().is_some() {
                    self.node.log("direct connections: no longer listening".to_string());
                    // What came in while it listened goes with it.
                    let taken: Vec<Arc<PeerConn>> = self
                        .direct
                        .links
                        .lock()
                        .expect("never poisoned")
                        .values()
                        .filter_map(|ends| ends.taken.clone())
                        .collect();
                    for conn in taken {
                        conn.close("this machine no longer takes direct connections".to_string());
                    }
                    let this = self.clone();
                    tokio::spawn(async move {
                        if let Err(why) = this.firewall(BTreeSet::new()).await {
                            this.node.log(format!("direct connections: closing this machine's firewall: {why}"));
                        }
                    });
                }
                self.direct.reported.lock().expect("never poisoned").clear();
                return;
            }
            listen::follow(self, listening.get_or_insert_with(Listening::default), &wanted)
        };
        // Each check the hub makes is answered once: a check ended is
        // forgotten, so the same addresses checked again are answered again.
        let trial = view.trial.clone();
        let report = {
            let mut reported = self.direct.reported.lock().expect("never poisoned");
            let report = !trial.is_empty() && *reported != trial;
            *reported = trial.clone();
            report
        };
        if !started && !report {
            return;
        }
        let this = self.clone();
        let ports: BTreeSet<u16> = wanted.iter().map(|(_, port)| *port).collect();
        tokio::spawn(async move {
            let outcome = this.open_ports(ports).await;
            if report {
                let question = crate::wire::Question::Listening { at: trial, outcome };
                if let Err(error) = this.ask(question).await {
                    this.node.log(format!("telling the hub this machine listens for its check: {error}"));
                }
            }
        });
    }

    /// Opens this machine's firewall at `ports` as far as the World does
    /// that, and says what came of listening and of the firewall.
    async fn open_ports(self: &Arc<Self>, ports: BTreeSet<u16>) -> Result<crate::wire::Firewall, String> {
        if let Some(Err(why)) = self.direct.listening() {
            return Err(why);
        }
        match self.firewall(ports).await {
            Ok(firewall) => {
                self.node.log(format!("direct connections: this machine's firewall: {firewall:?}"));
                Ok(firewall)
            }
            Err(why) => {
                self.node.log(format!("direct connections: opening this machine's firewall failed: {why}"));
                Err(why)
            }
        }
    }

    /// Brings this machine's firewall to let in exactly `ports`, or nothing.
    /// The changes are made one at a time, and each makes the firewall what
    /// was last asked: a change asked later is never undone by one asked
    /// earlier that finished later.
    async fn firewall(self: &Arc<Self>, ports: BTreeSet<u16>) -> Result<crate::wire::Firewall, String> {
        *self.direct.firewall.wanted.lock().expect("never poisoned") = ports;
        let (node, firewall) = (self.node.clone(), self.direct.firewall.clone());
        tokio::task::spawn_blocking(move || {
            let _one_at_a_time = firewall.applying.lock().expect("never poisoned");
            let wanted = firewall.wanted.lock().expect("never poisoned").clone();
            node.open_ports(&wanted)
        })
        .await
        .map_err(|error| format!("changing the firewall: {error}"))?
    }

    /// The machines this one keeps a link with: those that take direct
    /// connections and that one of the two may say something to, for some
    /// account (`world-network.md`, 直连). A right taken back lets the link
    /// go with the view that says so.
    fn wanted_links(&self, view: &View) -> BTreeMap<MachineName, Vec<SocketAddr>> {
        if !self.kind.is_computer() {
            return BTreeMap::new();
        }
        let me = &view.me;
        let i_listen = view.me().is_some_and(|me| !me.direct.is_empty());
        view.machines()
            .filter(|member| member.name != *me && !member.direct.is_empty())
            .filter(|member| may_talk(view, me, &member.name))
            // Two machines that both take direct connections link once: the
            // one whose name sorts first dials.
            .filter(|member| !i_listen || me.as_str() < member.name.as_str())
            .map(|member| (member.name.clone(), member.direct.clone()))
            .collect()
    }

    fn follow_keepers(self: &Arc<Self>, view: &View) {
        let mut wanted = self.wanted_links(view);
        // A phone links on demand: only with machines it has calls to.
        if !self.kind.is_computer() {
            let demand = self.direct.demand.lock().expect("never poisoned");
            for member in view.machines() {
                if !member.direct.is_empty() && demand.get(&member.name).is_some_and(|calls| *calls > 0) {
                    wanted.insert(member.name.clone(), member.direct.clone());
                }
            }
        }
        let mut keepers = self.direct.keepers.lock().expect("never poisoned");
        let gone: Vec<MachineName> = keepers
            .iter()
            .filter(|(peer, keeper)| wanted.get(*peer) != Some(&keeper.at))
            .map(|(peer, _)| peer.clone())
            .collect();
        for peer in gone {
            keepers.remove(&peer);
            if let Some(conn) =
                self.direct.links.lock().expect("never poisoned").get_mut(&peer).and_then(|ends| ends.dialed.take())
            {
                conn.close("this machine no longer keeps a direct link with it at that address".to_string());
            }
            if !wanted.contains_key(&peer) {
                self.direct.forget(&peer);
            }
        }
        for (peer, at) in wanted {
            if keepers.contains_key(&peer) {
                continue;
            }
            let task = tokio::spawn(peer::keep(self.clone(), peer.clone(), at.clone()));
            keepers.insert(peer, Keeper { at, task });
        }
    }

    /// Ends the links of machines no longer in the roster, and the calls
    /// being answered over links whose right was taken back.
    fn follow_links(self: &Arc<Self>, view: &View) {
        let conns: Vec<Arc<PeerConn>> = self
            .direct
            .links
            .lock()
            .expect("never poisoned")
            .values()
            .flat_map(|ends| [ends.dialed.clone(), ends.taken.clone()])
            .flatten()
            .collect();
        for conn in conns {
            if view.member(&conn.name).is_none_or(|member| member.key != conn.key) {
                conn.close(format!("{} is no longer a member this machine knows", conn.name));
            }
        }
        self.cut_lapsed(view);
    }

    /// Notes that a phone has a call in flight to `peer`, linking with it
    /// directly while it does; the guard ends the note.
    pub(crate) fn demand(self: &Arc<Self>, peer: &MachineName) -> Option<Demand> {
        if self.kind.is_computer() {
            return None;
        }
        let first = {
            let mut demand = self.direct.demand.lock().expect("never poisoned");
            let calls = demand.entry(peer.clone()).or_default();
            *calls += 1;
            *calls == 1
        };
        if first {
            self.follow_keepers(&self.view());
        }
        Some(Demand { inner: Arc::downgrade(self), peer: peer.clone() })
    }

    /// The direct link a call to `to` for `account` goes over, if it can go
    /// over one: a call that makes the hub note that a machine acts for the
    /// account goes through the hub, which notes it.
    pub(crate) fn direct_link_for(
        &self,
        to: &MachineName,
        account: &AccountName,
        call: &fastctx_types::remote::RemoteCall,
    ) -> Option<Arc<PeerConn>> {
        if matches!(
            call,
            fastctx_types::remote::RemoteCall::EnterProject { .. } | fastctx_types::remote::RemoteCall::Plan { .. }
        ) {
            return None;
        }
        let conn = self.direct.link(to)?;
        if crate::message::login_asked(call).is_some() && !self.view().may_act_for(account, to) {
            return None;
        }
        Some(conn)
    }
}

/// A phone's call in flight to a machine.
pub(crate) struct Demand {
    inner: std::sync::Weak<Inner>,
    peer: MachineName,
}

impl Drop for Demand {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else { return };
        let last = {
            let mut demand = inner.direct.demand.lock().expect("never poisoned");
            let calls = demand.entry(self.peer.clone()).or_default();
            *calls = calls.saturating_sub(1);
            if *calls == 0 {
                demand.remove(&self.peer);
                true
            } else {
                false
            }
        };
        if last {
            inner.follow_keepers(&inner.view());
        }
    }
}

/// Whether `a` and `b` may say anything to each other, either way, for any
/// account in `view`.
fn may_talk(view: &View, a: &MachineName, b: &MachineName) -> bool {
    let directory = &view.directory;
    directory.accounts.iter().any(|account| {
        [Purpose::Hands, Purpose::Agents, Purpose::Look, Purpose::Words].into_iter().any(|purpose| {
            directory.admits(a, &account.name, b, purpose).is_ok()
                || directory.admits(b, &account.name, a, purpose).is_ok()
        })
    })
}
