//! Reaching other machines (`world-network.md`): which hub a machine is on,
//! and which account a call or a letter from here speaks for.
//!
//! A machine is written as its name, or as `name.hub` where the name is on
//! more than one of the hubs this computer is on. Whom something here works
//! for (`Acting`) gives the accounts it may speak for on each hub; of those,
//! the first the directory lets say this to that machine is the one used.

use std::sync::Arc;

use fastctx_net::link::{CallFailure, Feed, Link, Status};
pub(crate) use fastctx_types::access::Written;
use fastctx_types::access::{Acting, HubAccount, HubId, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteResult};
use fastctx_types::{AccountName, MachineName};

use crate::Node;

/// How long an action waits for the view a hub gives a new connection.
const VIEW_COMING: std::time::Duration = std::time::Duration::from_secs(60);
/// How long it waits for a hub that is being connected to again.
const RECONNECTING: std::time::Duration = std::time::Duration::from_secs(10);

impl Node {
    /// The links to the hubs this computer is on.
    pub(crate) fn links(&self) -> Vec<Arc<Link>> {
        self.links.read().expect("never poisoned").values().cloned().collect()
    }

    pub(crate) fn link_of(&self, hub: &HubId) -> Option<Arc<Link>> {
        self.links.read().expect("never poisoned").get(hub).cloned()
    }

    /// Whether this computer is on any hub.
    pub(crate) fn connected_anywhere(&self) -> bool {
        !self.links.read().expect("never poisoned").is_empty()
    }

    /// The ways to other machines may have changed: a link said so, or one
    /// came or went with a login.
    pub(crate) fn ways_changed(&self) {
        self.ways.send_modify(|changes| *changes = changes.wrapping_add(1));
    }

    /// The ways there are now to `machines`, through every hub each is on.
    fn ways(&self, machines: &[Written]) -> Vec<fastctx_net::link::Way> {
        machines
            .iter()
            .filter_map(|machine| Some((machine, self.locate(machine).ok()?)))
            .flat_map(|(machine, links)| links.into_iter().filter_map(|link| link.way_to(&machine.name)))
            .collect()
    }

    /// Resolves once a way to one of `machines` opens that was not there
    /// when this was asked (`fastctx_net::again::reopened`): what broke on
    /// the way to them goes on at once then (`crate::again`).
    pub(crate) fn reopened(self: &Arc<Self>, machines: Vec<Written>) -> impl Future<Output = ()> + Send + 'static {
        let node = Arc::downgrade(self);
        fastctx_net::again::reopened(self.ways.subscribe(), move || match node.upgrade() {
            Some(node) => node.ways(&machines),
            None => Vec::new(),
        })
    }

    /// The links of the hubs `machine` is on. A computer has one name and
    /// one key on every hub it is on, so a name the views of several hubs
    /// hold for one key is one machine, reached through any of them; held
    /// for different keys, it is several, and must be written `name.hub`.
    pub(crate) fn locate(&self, machine: &Written) -> Result<Vec<Arc<Link>>, Failure> {
        let links = self.links();
        if links.is_empty() {
            return Err(Failure::not_run(format!(
                "{} is in no network, so it reaches no other machine",
                self.machine()
            )));
        }
        if let Some(hub) = &machine.hub {
            return links
                .into_iter()
                .find(|link| link.membership().hub_name == *hub)
                .map(|link| vec![link])
                .ok_or_else(|| Failure::not_run(format!("{} is not logged in to a hub named {hub}", self.machine())));
        }
        let holding: Vec<(Arc<Link>, fastctx_net::sign::PublicKey)> = links
            .iter()
            .filter_map(|link| link.view().member(&machine.name).map(|member| (link.clone(), member.key)))
            .collect();
        // The hubs are not named: whoever asks may not be on all of them
        // (`world-network.md`, 共享机器与智能体协助). The machines they may
        // use are listed to them as they are to be written.
        if self.is_me(&machine.name) && holding.iter().any(|(link, _)| !link.view().is_me(&machine.name)) {
            return Err(Failure::not_run(format!(
                "{} also names this computer; write a network machine with its hub, as `fastctx nodes` lists it",
                machine.name
            )));
        }
        match holding.first() {
            // Not in any view yet: the only hub, if there is one, may know
            // it, and its answer says so if not.
            None if links.len() == 1 => Ok(links),
            None => {
                Err(Failure::not_run(format!("no hub {} is on has a machine named {}", self.machine(), machine.name)))
            }
            Some((_, key)) if holding.iter().all(|(_, other)| other == key) => {
                Ok(holding.into_iter().map(|(link, _)| link).collect())
            }
            Some(_) => Err(Failure::not_run(format!(
                "{} names different computers on different hubs; write it with its hub, as `fastctx nodes` lists it",
                machine.name
            ))),
        }
    }

    /// How the machine `name` of `link`'s hub is written here: its name,
    /// with the hub only when another hub this computer is on has a
    /// different computer of that name.
    pub(crate) fn written(&self, link: &Link, name: &MachineName) -> Written {
        let key = link.view().member(name).map(|member| member.key);
        let shared = (self.is_me(name) && !link.view().is_me(name))
            || self.links().iter().any(|other| {
                other.membership().id != link.membership().id
                    && other.view().member(name).is_some_and(|member| Some(&member.key) != key.as_ref())
            });
        Written { name: name.clone(), hub: shared.then(|| link.membership().hub_name.clone()) }
    }

    /// Whether `machine` identifies this computer. A qualified name belongs
    /// to that hub's directory; an unqualified local name must not also name
    /// another computer on any of the connected hubs.
    pub(crate) fn is_here(&self, machine: &Written) -> bool {
        let links = self.links();
        if let Some(hub) = &machine.hub {
            return links
                .iter()
                .find(|link| link.hub_name() == *hub)
                .is_some_and(|link| link.view().is_me(&machine.name));
        }
        self.is_me(&machine.name)
            && links.iter().all(|link| {
                let view = link.view();
                view.member(&machine.name).is_none() || view.is_me(&machine.name)
            })
    }

    /// The accounts `acting` may speak for on `link`'s hub.
    pub(crate) fn accounts_on(&self, link: &Link, acting: &Acting) -> Vec<AccountName> {
        match acting {
            Acting::Here => link.view().me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default(),
            Acting::Accounts { accounts } => accounts
                .iter()
                .filter(|account| account.hub == link.membership().id)
                .map(|account| account.account.clone())
                .collect(),
        }
    }

    /// Whether `acting` works for `account` of `hub`.
    pub(crate) fn serves(&self, acting: &Acting, hub: &HubId, account: &AccountName) -> bool {
        match acting {
            Acting::Here => {
                self.link_of(hub).is_some_and(|link| link.view().me().is_some_and(|me| me.logins.contains(account)))
            }
            Acting::Accounts { accounts } => {
                accounts.contains(&HubAccount { hub: hub.clone(), account: account.clone() })
            }
        }
    }

    /// `town` as a call to a machine of `link`'s hub carries it: a project
    /// is of one hub (`world-network.md`), so what goes to a machine of
    /// another hub is kept outside any project there.
    pub(crate) fn town_across(
        &self,
        link: &Link,
        town: Option<&fastctx_types::TownName>,
    ) -> Result<Option<fastctx_types::TownName>, Failure> {
        let Some(town) = town else { return Ok(None) };
        let place = crate::ledger::names::place(&self.world, town)
            .map_err(|error| Failure::not_run(format!("reading the project {town}'s hub: {error}")))?;
        Ok(place.filter(|place| place.hub == link.membership().id).map(|_| town.clone()))
    }

    /// Whether scripts may start on this computer for `acting`: for a
    /// session the user opened here, or for an account logged in here or
    /// that may work here. An account only an agent's login here assists
    /// works elsewhere (`world-network.md`, 共享机器与智能体协助); when not,
    /// why.
    pub(crate) fn works_here(&self, acting: &Acting) -> Result<(), String> {
        self.authorize_here(acting).map_err(|failure| failure.message)
    }

    /// Waits until a hub `to` is on can judge an action for it now: while
    /// one is connected but has not given this connection its view yet, a
    /// minute at most, and while one is connecting again, a few seconds. An
    /// action asked in the moments after a machine connects, or while a large
    /// network's view is on its way, is judged once the view comes rather
    /// than refused for want of it (`authorization::current_view`).
    pub(crate) async fn settled(&self, to: &Written) {
        match self.locate(to) {
            Ok(links) => settle(&links, Settled::Any).await,
            Err(_) if to.hub.is_none() => {
                // An unknown unqualified name may be in any view still coming.
                // One unrelated hub already answering cannot settle that absence.
                settle(&self.links(), Settled::Every).await;
            }
            Err(_) => {}
        }
    }

    /// Waits, as `settled` does, until the hubs of the accounts `acting`
    /// works for can each judge for it now: what is decided here for them,
    /// and kept (whether a conversation assists another account), is decided
    /// on the hub's word rather than guessed.
    pub(crate) async fn settled_for(&self, acting: &Acting) {
        let Acting::Accounts { accounts } = acting else { return };
        let links: Vec<Arc<Link>> = accounts.iter().filter_map(|account| self.link_of(&account.hub)).collect();
        settle(&links, Settled::Every).await;
    }

    /// Whether `acting` may work on this computer, as its hubs now say; an
    /// error while one has not given this connection its view yet, when a
    /// judgement would be a guess (`authorization::current_view`).
    pub(crate) fn may_work_here(&self, acting: &Acting) -> Result<bool, Failure> {
        match self.authorize_here(acting) {
            Ok(()) => Ok(true),
            Err(failure) if crate::authorization::view_pending(&failure) => Err(failure),
            Err(_) => Ok(false),
        }
    }

    /// Calls `to`, for whom `acting` works, asking `purpose` of it: for each
    /// account `acting` may speak for there that may, in turn, until one is
    /// let in. What a call about a resident there may do depends on whom the
    /// resident works for, which only that machine knows; a call turned
    /// away at its door ran nothing, so the next account may ask again.
    pub(crate) async fn call(
        &self,
        acting: &Acting,
        to: &Written,
        purpose: Purpose,
        call: RemoteCall,
    ) -> Result<RemoteResult, CallFailure> {
        self.settled(to).await;
        let mut turned_away = None;
        for (link, account) in self.speakers(acting, to, purpose)? {
            match link.call(&to.name, &account, purpose, call.clone()).await {
                Ok(Err(failure)) if failure.not_run => turned_away = Some(failure),
                answered => return answered,
            }
        }
        Ok(Err(turned_away.expect("there is a speaker to ask")))
    }

    /// Calls `to` with a data connection, as `call` does, with a feed
    /// `feed` makes for each try.
    pub(crate) async fn call_with_stream(
        &self,
        acting: &Acting,
        to: &Written,
        purpose: Purpose,
        call: RemoteCall,
        feed: impl Fn() -> Feed,
    ) -> Result<RemoteResult, CallFailure> {
        self.settled(to).await;
        let mut turned_away = None;
        for (link, account) in self.speakers(acting, to, purpose)? {
            match link.call_with_stream(&to.name, &account, purpose, call.clone(), feed()).await {
                Ok(Err(failure)) if failure.not_run => turned_away = Some(failure),
                answered => return answered,
            }
        }
        Ok(Err(turned_away.expect("there is a speaker to ask")))
    }

    /// The link and the account to say something for `purpose` to `to`
    /// that is not answered: the first `speakers` gives, once a hub `to` is
    /// on can judge it (`settled`).
    pub(crate) async fn speaking(
        &self,
        acting: &Acting,
        to: &Written,
        purpose: Purpose,
    ) -> Result<(Arc<Link>, AccountName), CallFailure> {
        self.settled(to).await;
        Ok(self.speakers(acting, to, purpose)?.into_iter().next().expect("speakers names one at least"))
    }

    /// A cached route is sufficient to queue mail. This grants no execution
    /// permission: the hub checks the current account before accepting it,
    /// and its recipient checks admission before taking custody.
    pub(crate) fn mailbox(
        &self,
        acting: &Acting,
        to: &Written,
        purpose: Purpose,
    ) -> Result<(Arc<Link>, AccountName), CallFailure> {
        let links = self
            .locate(to)
            .map_err(|failure| CallFailure::Unreachable { name: to.name.clone(), reason: failure.message })?;
        for link in links {
            let view = link.view();
            for account in self.accounts_on(&link, acting) {
                if purpose == Purpose::Echo || view.admits(&view.me, &account, &to.name, purpose).is_ok() {
                    return Ok((link.clone(), account));
                }
            }
        }
        Err(CallFailure::NotAllowed {
            name: to.name.clone(),
            reason: "no remembered account route can post this letter".into(),
        })
    }

    /// The hubs `to` is on, each with the accounts `acting` may say
    /// something for `purpose` to it for, at least one. News goes for the
    /// accounts it is about without a look at this machine's view, which
    /// need not show the resident's machine; the hub checks it.
    pub(crate) fn speakers(
        &self,
        acting: &Acting,
        to: &Written,
        purpose: Purpose,
    ) -> Result<Vec<(Arc<Link>, AccountName)>, CallFailure> {
        let unreachable =
            |failure: Failure| CallFailure::Unreachable { name: to.name.clone(), reason: failure.message };
        let links = self.locate(to).map_err(unreachable)?;
        let mut speakers = Vec::new();
        let mut why = Vec::new();
        for link in links {
            let accounts = self.accounts_on(&link, acting);
            let hub = &link.membership().hub_name;
            if accounts.is_empty() {
                why.push(format!("this works for no account of the hub {hub}, which {} is on", to.name));
                continue;
            }
            let view = match crate::authorization::current_view(&link) {
                Ok(view) => view,
                Err(failure) => {
                    why.push(failure.message);
                    continue;
                }
            };
            // This computer as that hub calls it.
            let here = &view.me;
            for account in accounts {
                match view.admits(here, &account, &to.name, purpose) {
                    _ if purpose == Purpose::Echo => speakers.push((link.clone(), account)),
                    Ok(()) => speakers.push((link.clone(), account)),
                    Err(refused) => why.push(refused),
                }
            }
        }
        if speakers.is_empty() {
            return Err(CallFailure::NotAllowed { name: to.name.clone(), reason: why.join("; ") });
        }
        Ok(speakers)
    }
}

/// Whom `settle` waits for: any one of the links, as a machine on several
/// hubs is reached through whichever can judge; or every one, as each
/// account is judged on its own hub.
#[derive(Clone, Copy)]
enum Settled {
    Any,
    Every,
}

/// Waits while `links` may soon judge for their hubs: a link connected
/// whose view of this connection has not come yet, a minute at most, and
/// one connecting again, a few seconds. A stopped link is not waited for.
async fn settle(links: &[Arc<Link>], settled: Settled) {
    let began = tokio::time::Instant::now();
    let current = |link: &Arc<Link>| matches!(link.status(), Status::Online { .. }) && link.view_current();
    loop {
        let done = match settled {
            Settled::Any => links.iter().any(current),
            Settled::Every => links.iter().all(current),
        };
        if done {
            return;
        }
        let waited = began.elapsed();
        let coming = links.iter().filter(|link| !current(link)).any(|link| match link.status() {
            Status::Online { .. } => waited < VIEW_COMING,
            Status::Connecting { .. } => waited < RECONNECTING,
            Status::Stopped { .. } => false,
        });
        if !coming {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
