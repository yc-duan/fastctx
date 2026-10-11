//! A user's device, such as a phone: its own key and a link to each hub it
//! is logged in to, with no node, agent host, command runner or ledger
//! replica. A device only logs in; it is never registered as a machine.

mod around;
mod calls;
mod connection_state;
pub(crate) mod file_read;
mod file_search;
mod lines;
mod store;
mod upload;
pub(crate) mod watch;

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
    sessions: RwLock<BTreeMap<HubId, Arc<Session>>>,
    lifecycle: tokio::sync::Mutex<()>,
    managed: bool,
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
    /// Ends this application's connections while retaining every login.
    /// Uninstall uses this before a separate withdrawal connection or exit.
    pub fn disconnect(&self) {
        for session in self.sessions() {
            session.link.stop();
        }
    }
    /// Opens an identity and uses the platform's network/resume notifications.
    /// Must run inside a Tokio runtime.
    pub fn open(directory: PathBuf) -> io::Result<Arc<Self>> {
        Self::open_with_notifications(directory, false)
    }

    /// Opens an identity whose application delivers network/resume callbacks
    /// through [`Device::reconnect`], as on Android. Heartbeats and retries
    /// remain active even without callbacks.
    pub fn open_managed(directory: PathBuf) -> io::Result<Arc<Self>> {
        Self::open_with_notifications(directory, true)
    }

    fn open_with_notifications(directory: PathBuf, managed: bool) -> io::Result<Arc<Self>> {
        let store = Arc::new(Store::open(directory)?);
        let memberships = store.memberships()?;
        let device = Arc::new(Self {
            store,
            sessions: RwLock::new(BTreeMap::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            managed,
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

    async fn logged_in(&self, joined: Joined) -> Result<DeviceJoined, Error> {
        let Joined { hub, hub_name, name, account, addresses, cert } = joined;
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
                };
                self.store.keep(&membership)?;
                self.start(membership)?;
            }
        }
        Ok(DeviceJoined { hub: hub_name, name, account })
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
            if answer.is_err() && !matches!(session.link.status(), Status::Stopped { .. }) {
                answer.map_err(failed)?;
            }
            let id = session.link.membership().id.clone();
            device.sessions.write().expect("device sessions").remove(&id);
            drop(session);
            device.store.forget(&id)?;
            Ok(())
        })
        .await
        .map_err(failed)?
    }

    fn start(&self, membership: Membership) -> io::Result<()> {
        let start = if self.managed { Link::start_managed } else { Link::start };
        let key = self.store.key()?;
        let id = membership.id.clone();
        let arrivals = self.store.arrivals(&id);
        let handler = Arc::new(Handler { _store: self.store.clone(), link: std::sync::OnceLock::new(), arrivals });
        let link = Arc::new(start(self.store.net(&id), membership, key, handler.clone())?);
        let _ = handler.link.set(Arc::downgrade(&link));
        self.sessions.write().expect("device sessions").insert(id, Arc::new(Session { link }));
        Ok(())
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

    /// The session of the hub `machine` is on.
    fn reaching(&self, machine: &MachineName) -> Result<Arc<Session>, Error> {
        let sessions = self.sessions();
        if sessions.is_empty() {
            return Err(failed("this device is logged in to no hub"));
        }
        let holding: Vec<Arc<Session>> =
            sessions.into_iter().filter(|session| session.link.view().member(machine).is_some()).collect();
        match holding.len() {
            1 => Ok(holding.into_iter().next().expect("one")),
            0 => Err(failed(format!("no hub this device is on has a machine named {machine}"))),
            _ => Err(failed(format!("{machine} is a machine on more than one hub this device is on"))),
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

struct Handler {
    // Watches can outlive World; cancelled network tasks can still be
    // finishing disk writes. The link's handler lives with its last task,
    // so the identity remains locked until all its network work has ended.
    _store: Arc<Store>,
    /// The link it handles for, set once it is made.
    link: std::sync::OnceLock<std::sync::Weak<Link>>,
    /// Where who came into this device's accounts there is kept.
    arrivals: std::path::PathBuf,
}

impl fastctx_net::link::Node for Handler {
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

    fn ledger_changed(&self, _: Option<fastctx_types::access::TownId>) {}

    fn directory_changed(&self) {
        let Some(link) = self.link.get().and_then(std::sync::Weak::upgrade) else { return };
        if let Err(error) = fastctx_net::arrivals::notice(&self.arrivals, &link.view()) {
            eprintln!("World device: keeping who came into this device's accounts: {error}");
        }
    }

    fn log(&self, line: String) {
        eprintln!("World device: {line}");
    }
}
