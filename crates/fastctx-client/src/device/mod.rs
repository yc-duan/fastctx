//! A user's device, such as a phone: its own key and a link to each hub it
//! is logged in to, with no node, agent host, command runner or ledger
//! replica. A device only logs in; it is never registered as a machine.

mod around;
mod attachment_places;
mod calls;
mod connection_state;
mod features;
pub(crate) mod file_read;
mod file_search;
mod lines;
mod mcp;
mod project_memory;
mod store;
mod thumbnail;
mod updates;
mod upload;
pub(crate) mod watch;
mod world_now;

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use fastctx_net::directory::Kind;
use fastctx_net::ids::HubId;
use fastctx_net::link::{BoxFuture, Caller, Link, Membership, Status};
use fastctx_net::login::{Device as Joining, Joined};
use fastctx_types::access::Purpose;
pub use fastctx_types::device::{DeviceHub, DeviceJoined, DeviceState};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::{AccountName, HubName, MachineName};

use crate::world::Error;
use store::Store;

/// The application supplies the directory. Opening it resumes every
/// membership and starts reconnecting; cloning the World shares its links.
pub struct Device {
    store: Arc<Store>,
    /// What it keeps of the conversations it showed, which it forgets of
    /// one deleted (`fastctx_net::wire::Down::Forget`).
    cache: Arc<crate::cache::Cache>,
    sessions: RwLock<BTreeMap<HubId, Arc<Session>>>,
    lifecycle: tokio::sync::Mutex<()>,
    managed: bool,
    features: features::Switches,
    /// Moves whenever the ways to the machines may have changed, on any of
    /// its links (`fastctx_net::link::Way`).
    ways: Arc<tokio::sync::watch::Sender<u64>>,
    /// What it keeps to show the World without asking (`world_now`).
    shown: world_now::Shown,
    /// Moves whenever what it shows of the World may have changed.
    shown_changes: Arc<tokio::sync::watch::Sender<u64>>,
    /// New versions of Fastctx here (`updates`).
    updates: updates::Updates,
}

pub(super) struct Session {
    link: Arc<Link>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.link.stop();
    }
}

impl Device {
    pub(crate) fn record_watch(
        &self,
        hub: Option<&HubName>,
        account: Option<AccountName>,
        kind: String,
    ) -> Result<fastctx_net::link::account_records::RecordWatch, Error> {
        let session = self.on_hub(hub)?;
        let account = calls::profile_account(&session, account)?;
        session.link.watch_account_records(account, kind).map_err(failed)
    }

    /// Ends this application's connections while retaining every login.
    /// Uninstall uses this before a separate withdrawal connection or exit.
    pub fn disconnect(&self) {
        for session in self.sessions() {
            session.link.stop();
        }
    }
    /// Opens an identity and uses the platform's network/resume notifications;
    /// `cache` is what it keeps of the conversations it shows. Must run
    /// inside a Tokio runtime.
    pub fn open(directory: PathBuf, cache: Arc<crate::cache::Cache>) -> io::Result<Arc<Self>> {
        Self::open_with_notifications(directory, cache, false)
    }

    /// Opens an identity whose application delivers network/resume callbacks
    /// through [`Device::reconnect`], as on Android. Heartbeats and retries
    /// remain active even without callbacks.
    pub fn open_managed(directory: PathBuf, cache: Arc<crate::cache::Cache>) -> io::Result<Arc<Self>> {
        Self::open_with_notifications(directory, cache, true)
    }

    fn open_with_notifications(
        directory: PathBuf,
        cache: Arc<crate::cache::Cache>,
        managed: bool,
    ) -> io::Result<Arc<Self>> {
        let store = Arc::new(Store::open(directory)?);
        let memberships = store.memberships()?;
        let device = Arc::new(Self {
            store,
            cache,
            sessions: RwLock::new(BTreeMap::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            managed,
            features: features::Switches::default(),
            ways: Arc::new(tokio::sync::watch::channel(0).0),
            shown: world_now::Shown::default(),
            shown_changes: Arc::new(tokio::sync::watch::channel(0).0),
            updates: updates::Updates::default(),
        });
        for membership in memberships {
            device.start(membership)?;
        }
        Ok(device)
    }

    pub fn directory(&self) -> &Path {
        self.store.root()
    }

    pub fn state(&self) -> DeviceState {
        let sessions = self.sessions();
        DeviceState {
            name: sessions.first().map(|session| session.link.membership().name.clone()),
            hubs: sessions
                .iter()
                .map(|session| DeviceHub {
                    hub: session.link.hub_name(),
                    link: session.link.state(),
                    logins: session.link.view().me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default(),
                })
                .collect(),
        }
    }

    /// The native layer calls this on network changes and on returning to
    /// the foreground. It cannot revive a removed membership.
    pub fn reconnect(&self) {
        for session in self.sessions() {
            session.link.reconnect();
        }
    }

    /// Logs this device in to `account` on the hub at `hub`. Cancellation
    /// of the UI request does not abandon the login.
    pub async fn login(
        self: &Arc<Self>,
        hub: Vec<SocketAddr>,
        account: AccountName,
        password: String,
        name: Option<MachineName>,
    ) -> Result<DeviceJoined, Error> {
        let device = self.clone();
        tokio::spawn(async move {
            let _changing = device.lifecycle.lock().await;
            let key = device.store.key()?;
            let joining = Joining { name: device.name(name)?, key: &key, kind: Kind::Phone, register: false };
            let joined = fastctx_net::login::with_password(&hub, &account, &password, joining).await.map_err(failed)?;
            device.logged_in(joined).await
        })
        .await
        .map_err(failed)?
    }

    /// Logs this device in with an enrollment token, as a phone scanning a
    /// QR code does.
    pub async fn login_with_token(
        self: &Arc<Self>,
        token: String,
        name: Option<MachineName>,
    ) -> Result<DeviceJoined, Error> {
        let device = self.clone();
        tokio::spawn(async move {
            let _changing = device.lifecycle.lock().await;
            let token = fastctx_net::token::Token::parse(&token).map_err(failed)?;
            let key = device.store.key()?;
            let joining = Joining { name: device.name(name)?, key: &key, kind: Kind::Phone, register: false };
            let joined = fastctx_net::login::with_token(&token, joining).await.map_err(failed)?;
            device.logged_in(joined).await
        })
        .await
        .map_err(failed)?
    }

    /// The name this device logs in under: the one it has on its hubs, or
    /// the one asked for while it is on none.
    fn name(&self, asked: Option<MachineName>) -> Result<MachineName, Error> {
        let existing = self.sessions().first().map(|session| session.link.membership().name.clone());
        match (existing, asked) {
            (Some(existing), Some(asked)) if existing != asked => Err(failed(format!(
                "this device is {existing} on the hubs it is on, and takes the same name on every hub"
            ))),
            (Some(existing), _) => Ok(existing),
            (None, Some(asked)) => Ok(asked),
            (None, None) => Err(failed("a device logging in for the first time needs a name, such as `phone`")),
        }
    }

    async fn logged_in(self: &Arc<Self>, joined: Joined) -> Result<DeviceJoined, Error> {
        let Joined { hub, hub_name, name, account, addresses, cert, relays } = joined;
        match self.session(&hub) {
            Some(session) => session.link.refresh().await.map_err(failed)?,
            None => {
                let membership = Membership {
                    id: hub.clone(),
                    hub_name: hub_name.clone(),
                    name: name.clone(),
                    addresses,
                    cert,
                    kind: Kind::Phone,
                    relays,
                };
                self.store.keep(&membership)?;
                self.start(membership)?;
            }
        }
        Ok(DeviceJoined { hub: hub_name, name, account })
    }

    /// Leaves the network `hub`, named `name`, which its administrator
    /// dissolved, as a logout leaves it; the user is told once. The notice
    /// is kept first, so nothing shows the network gone without it.
    async fn left_dissolved(&self, hub: &HubId, name: &HubName) -> io::Result<()> {
        let _changing = self.lifecycle.lock().await;
        self.store.note_dissolved(name)?;
        let Some(session) = self.sessions.write().expect("device sessions").remove(hub) else { return Ok(()) };
        drop(session);
        changed(&self.ways);
        changed(&self.shown_changes);
        self.store.forget(hub)
    }

    /// Logs this device out of `account` on the hub `hub`, or the one hub;
    /// with no login left there, the link stops and its records go aside.
    pub async fn logout(self: &Arc<Self>, hub: Option<HubName>, account: AccountName) -> Result<(), Error> {
        let device = self.clone();
        tokio::spawn(async move {
            let _changing = device.lifecycle.lock().await;
            let session = device.on_hub(hub.as_ref())?;
            let last = session.link.view().me().is_none_or(|me| me.logins.iter().all(|login| *login == account));
            let answer = session.link.account_change(&account, fastctx_types::accounts::AccountChange::Logout).await;
            if !last {
                return answer.map(|_| ()).map_err(failed);
            }
            // Stopped, the hub ended the session first; unreachable, it is
            // not told, and the device leaves it all the same.
            if answer.is_err() && matches!(session.link.status(), Status::Online { .. }) {
                answer.map_err(failed)?;
            }
            let id = session.link.membership().id.clone();
            device.sessions.write().expect("device sessions").remove(&id);
            drop(session);
            changed(&device.ways);
            // What the device shows changes with it: a network fewer.
            changed(&device.shown_changes);
            device.store.forget(&id)?;
            Ok(())
        })
        .await
        .map_err(failed)?
    }

    fn start(self: &Arc<Self>, membership: Membership) -> io::Result<()> {
        let start = if self.managed { Link::start_managed } else { Link::start };
        let key = self.store.key()?;
        let id = membership.id.clone();
        let arrivals = self.store.arrivals(&id);
        let handler = Arc::new(Handler {
            _store: self.store.clone(),
            device: Arc::downgrade(self),
            cache: self.cache.clone(),
            link: std::sync::OnceLock::new(),
            arrivals,
            ways: self.ways.clone(),
            shown: self.shown_changes.clone(),
            towns_stale: self.shown.stale.clone(),
            projects: std::sync::Mutex::new(None),
        });
        let link = Arc::new(start(self.store.net(&id), membership, key, handler.clone())?);
        let _ = handler.link.set(Arc::downgrade(&link));
        self.sessions.write().expect("device sessions").insert(id, Arc::new(Session { link }));
        // What the link said of its ways before it was among the sessions
        // was not seen in them.
        changed(&self.ways);
        self.report_versions();
        Ok(())
    }

    /// Resolves once a way to `machine` opens that was not there when this
    /// was asked, as a pause before trying again waits for one
    /// (`crate::again`).
    pub(crate) fn reopened(
        self: &Arc<Self>,
        machine: &fastctx_types::access::Written,
    ) -> impl Future<Output = ()> + Send + 'static {
        let (device, machine) = (Arc::downgrade(self), machine.clone());
        fastctx_net::again::reopened(self.ways.subscribe(), move || match device.upgrade() {
            Some(device) => device
                .reaching(&machine)
                .ok()
                .into_iter()
                .filter_map(|session| session.link.way_to(&machine.name))
                .collect(),
            None => Vec::new(),
        })
    }

    fn sessions(&self) -> Vec<Arc<Session>> {
        self.sessions.read().expect("device sessions").values().cloned().collect()
    }

    fn session(&self, hub: &HubId) -> Option<Arc<Session>> {
        self.sessions.read().expect("device sessions").get(hub).cloned()
    }

    /// The session of the hub named `hub`, or of the one hub.
    fn on_hub(&self, hub: Option<&HubName>) -> Result<Arc<Session>, Error> {
        let sessions = self.sessions();
        match hub {
            Some(hub) => sessions
                .into_iter()
                .find(|session| session.link.hub_name() == *hub)
                .ok_or_else(|| failed(format!("this device is not logged in to a hub named {hub}"))),
            None => match sessions.len() {
                0 => Err(failed("this device is logged in to no hub")),
                1 => Ok(sessions.into_iter().next().expect("one")),
                _ => Err(failed("this device is on more than one hub; name one")),
            },
        }
    }

    /// The session of the named hub, or one of the hubs that know the same
    /// computer. Different computers sharing a name require a hub qualifier.
    fn reaching(&self, machine: &fastctx_types::access::Written) -> Result<Arc<Session>, Error> {
        if let Some(hub) = &machine.hub {
            return self.on_hub(Some(hub));
        }
        let sessions = self.sessions();
        if sessions.is_empty() {
            return Err(failed("this device is logged in to no hub"));
        }
        let holding: Vec<_> = sessions
            .into_iter()
            .filter_map(|session| {
                let key = session.link.view().member(&machine.name)?.key;
                Some((session, key))
            })
            .collect();
        match holding.first() {
            Some((session, key)) if holding.iter().all(|(_, other)| other == key) => Ok(session.clone()),
            None => Err(failed(format!("no hub this device is on has a machine named {machine}"))),
            Some(_) => Err(failed(format!(
                "{machine} names different computers; write it as {}",
                holding
                    .iter()
                    .map(|(session, _)| format!("{}.{}", machine.name, session.link.hub_name()))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ))),
        }
    }
}

impl Session {
    /// The account this device speaks for to ask `purpose` of `machine`.
    pub(super) fn account_for(&self, machine: &MachineName, purpose: Purpose) -> Result<AccountName, Error> {
        let view = self.link.view();
        let me = self.link.membership().name.clone();
        let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
        view.account_for(&logins, &me, machine, purpose).ok_or_else(|| {
            let why: Vec<String> =
                logins.iter().filter_map(|account| view.admits(&me, account, machine, purpose).err()).collect();
            failed(if why.is_empty() {
                format!("this device is logged in to no account that may use {machine}")
            } else {
                why.join("; ")
            })
        })
    }
}

fn failed(reason: impl std::fmt::Display) -> Error {
    crate::CallError::Failed(Failure::new(reason.to_string())).into()
}

/// The ways to the machines may have changed.
fn changed(ways: &tokio::sync::watch::Sender<u64>) {
    ways.send_modify(|changes| *changes = changes.wrapping_add(1));
}

struct Handler {
    // Watches can outlive World; cancelled network tasks can still be
    // finishing disk writes. The link's handler lives with its last task,
    // so the identity remains locked until all its network work has ended.
    _store: Arc<Store>,
    /// The device, to leave a network dissolved.
    device: std::sync::Weak<Device>,
    /// The device's (`Device::cache`).
    cache: Arc<crate::cache::Cache>,
    /// The link it handles for, set once it is made.
    link: std::sync::OnceLock<std::sync::Weak<Link>>,
    /// Where who came into this device's accounts there is kept.
    arrivals: std::path::PathBuf,
    /// The device's, moved when this link's ways may have changed.
    ways: Arc<tokio::sync::watch::Sender<u64>>,
    /// The device's, moved when what it shows of the World may have changed.
    shown: Arc<tokio::sync::watch::Sender<u64>>,
    /// The device's, set when its projects are to be listed again.
    towns_stale: Arc<std::sync::atomic::AtomicBool>,
    /// The projects the view named when it was last seen.
    projects: std::sync::Mutex<Option<Vec<(fastctx_types::access::TownId, fastctx_types::TownName)>>>,
}

impl Handler {
    /// The projects are to be listed again.
    fn towns_changed(&self) {
        self.towns_stale.store(true, std::sync::atomic::Ordering::Release);
        changed(&self.shown);
    }
}

impl fastctx_net::link::Node for Handler {
    fn place(
        &self,
        _: &MachineName,
        _: &fastctx_types::AccountName,
        _: fastctx_types::contract::Pool,
    ) -> Result<Box<dyn Send>, Failure> {
        Err(Failure::new("this member is a device; nothing runs on it"))
    }

    fn answering(&self, _: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        Err(Failure::new("this member is a device; nothing runs on it"))
    }

    fn call(&self, _: Caller, _: RemoteCall, _: Option<fastctx_net::data::Stream>) -> BoxFuture<RemoteResult> {
        Box::pin(async { Err(Failure::new("this member is a device; it accepts no incoming calls")) })
    }

    fn letter(
        &self,
        _: Caller,
        _: Purpose,
        _: fastctx_net::message::LetterId,
        _: Letter,
    ) -> BoxFuture<io::Result<Result<(), String>>> {
        Box::pin(async { Ok(Err("a device receives no letters".to_string())) })
    }

    fn ledger_changed(&self, _: Option<fastctx_types::access::TownId>) {
        self.towns_changed();
    }

    fn directory_changed(&self) {
        let Some(link) = self.link.get().and_then(std::sync::Weak::upgrade) else { return };
        let view = link.view();
        if let Err(error) = fastctx_net::arrivals::notice(&self.arrivals, &view) {
            eprintln!("World device: keeping who came into this device's accounts: {error}");
        }
        let projects: Vec<_> =
            view.projects.iter().map(|project| (project.town.clone(), project.name.clone())).collect();
        let renamed = self.projects.lock().expect("never poisoned").replace(projects.clone()) != Some(projects);
        if renamed {
            self.towns_changed();
        } else {
            changed(&self.shown);
        }
    }

    fn connected(&self) {
        self.towns_changed();
    }

    /// A network dissolved is left at once, as a computer leaves it
    /// (`world-network.md`, 解散网络).
    fn place_ended(&self, dissolved: bool, _then: Option<fastctx_types::uninstall::Then>) {
        let Some(device) = self.device.upgrade().filter(|_| dissolved) else { return };
        let Some(link) = self.link.get().and_then(std::sync::Weak::upgrade) else { return };
        let (hub, name) = (link.membership().id.clone(), link.hub_name());
        drop(link);
        tokio::spawn(async move {
            if let Err(error) = device.left_dissolved(&hub, &name).await {
                eprintln!("World device: leaving the dissolved network {name}: {error}");
            }
        });
    }

    fn ways_changed(&self) {
        changed(&self.ways);
        changed(&self.shown);
    }

    fn afoot_changed(&self) {
        changed(&self.shown);
    }

    fn versions_changed(&self) {
        changed(&self.shown);
    }

    /// A device keeps nothing of a conversation but the copy it showed,
    /// under any name its machine had.
    fn forget(&self, forgetting: fastctx_net::wire::Forgetting) -> BoxFuture<fastctx_net::wire::Forgot> {
        let conversation = forgetting.conversation;
        let mut machines = vec![conversation.machine.clone()];
        if let Some(link) = self.link.get().and_then(std::sync::Weak::upgrade)
            && let Some(member) = link.view().member(&conversation.machine)
        {
            machines.push(member.name.clone());
            machines.extend(member.was.iter().cloned());
        }
        machines.sort();
        machines.dedup();
        let cache = self.cache.clone();
        Box::pin(async move {
            let resident = conversation.resident;
            let forgot = tokio::task::spawn_blocking(move || {
                machines.into_iter().try_for_each(|machine| {
                    cache.forget(&fastctx_types::Address { machine, resident: resident.clone() })
                })
            })
            .await;
            match forgot {
                Ok(Ok(())) => fastctx_net::wire::Forgot::Done,
                Ok(Err(error)) => fastctx_net::wire::Forgot::Failed {
                    reason: format!("forgetting the copy this device kept of it: {error}"),
                },
                Err(error) => fastctx_net::wire::Forgot::Failed { reason: error.to_string() },
            }
        })
    }

    fn log(&self, line: String) {
        eprintln!("World device: {line}");
    }
}
