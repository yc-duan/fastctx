//! A user's device: its own identity and a persistent link, with no node,
//! agent host, command runner or ledger replica.

mod around;
mod calls;
mod connection_state;
pub(crate) mod file_read;
mod store;
mod upload;
pub(crate) mod watch;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use fastctx_net::code::Code;
use fastctx_net::link::{BoxFuture, Keys, Link, Membership, Status};
use fastctx_net::sign::PublicKey;
use fastctx_net::trust::Kind;
use fastctx_types::MachineName;
pub use fastctx_types::device::DeviceState;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::world::LinkState;

use crate::world::Error;
use store::{Identity, Store};

/// The application supplies the directory. Opening it resumes its identity
/// and starts reconnecting; cloning the World shares that single link.
pub struct Device {
    store: Arc<Store>,
    session: RwLock<Option<Arc<Session>>>,
    lifecycle: tokio::sync::Mutex<()>,
    managed: bool,
}

pub(super) struct Session {
    link: Arc<Link>,
    key: PublicKey,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.link.stop();
    }
}

impl Device {
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
        let identity = store.load()?;
        let device =
            Arc::new(Self { store, session: RwLock::new(None), lifecycle: tokio::sync::Mutex::new(()), managed });
        if let Some(identity) = identity {
            let keys = device.store.keys(&identity)?;
            device.start(identity, keys)?;
        }
        Ok(device)
    }

    pub fn directory(&self) -> &Path {
        self.store.root()
    }

    pub fn state(&self) -> DeviceState {
        match self.session.read().expect("device session").as_ref() {
            Some(session) => DeviceState {
                name: Some(session.link.membership().name.clone()),
                world: Some(session.link.membership().world.to_string()),
                link: link_state(&session.link),
            },
            None => DeviceState { name: None, world: None, link: LinkState::Alone },
        }
    }

    /// The native layer calls this on network changes and on returning to
    /// the foreground. It cannot revive a revoked membership.
    pub fn reconnect(&self) {
        if let Some(session) = self.session.read().expect("device session").as_ref() {
            session.link.reconnect();
        }
    }

    /// Joins only with a device invitation. A name fixed by the invitation
    /// is used unless the caller supplies the same name. Otherwise a name
    /// is required. Cancellation of the UI request does not abandon admission.
    pub async fn join(
        self: &Arc<Self>,
        code: String,
        name: Option<MachineName>,
    ) -> Result<(String, MachineName), Error> {
        let device = self.clone();
        tokio::spawn(async move { device.join_inner(code, name).await }).await.map_err(failed)?
    }

    async fn join_inner(&self, text: String, name: Option<MachineName>) -> Result<(String, MachineName), Error> {
        let _changing = self.lifecycle.lock().await;
        let code = Code::parse(&text).map_err(failed)?;
        let Code::Invitation { founder, .. } = &code else {
            return Err(failed("a device joins with a device invitation, not a founding code"));
        };
        let of_this_code = |membership: &Membership| {
            membership.world == *code.world() && membership.hub == code.hub() && membership.cert == code.cert()
        };
        let existing = self.store.load()?;
        self.restore()?;
        let retrying = existing.is_some();
        // An identity made by an earlier try of this code keeps its name and
        // keys: its admission may have happened without its reply arriving.
        let (earlier, directory, keys) = match existing {
            Some(identity) if !identity.pending && of_this_code(&identity.membership) => {
                self.connected()?.check_identity().await?;
                return Ok((identity.membership.world.to_string(), identity.membership.name));
            }
            Some(mut identity) if identity.pending && of_this_code(&identity.membership) => {
                let session = self.connected()?;
                if session.check_identity().await.is_ok() {
                    identity.pending = false;
                    self.store.save(Some(&identity))?;
                    return Ok((identity.membership.world.to_string(), identity.membership.name));
                }
                session.link.stop();
                *self.session.write().expect("device session") = None;
                let keys = self.store.keys(&identity)?;
                (Some(identity), None, keys)
            }
            Some(_) => return Err(failed("this device has a World identity already; leave it before joining another")),
            None => {
                let (directory, keys) = self.store.create()?;
                (None, Some(directory), keys)
            }
        };
        let asking = fastctx_net::join::Asking {
            name: earlier.as_ref().map(|identity| identity.membership.name.clone()).or(name),
            fallback: None,
            kind: Kind::Device,
        };
        let adopted = std::sync::Mutex::new(earlier);
        let before = |chosen: &MachineName| -> Result<(), String> {
            let mut adopted = adopted.lock().expect("never poisoned");
            if adopted.is_none() {
                let membership = Membership {
                    world: code.world().clone(),
                    name: chosen.clone(),
                    founder: *founder,
                    hub: code.hub().to_vec(),
                    cert: code.cert(),
                };
                let directory = directory.clone().expect("a new identity has its directory");
                *adopted = Some(self.store.adopt(directory, membership).map_err(|error| error.to_string())?);
            }
            Ok(())
        };
        let admission = fastctx_net::join::attempt(&code, asking, &keys, before).await;
        let identity = adopted.into_inner().expect("never poisoned");
        match (admission, identity) {
            (Err(fastctx_net::join::JoinError::Refused(reason) | fastctx_net::join::JoinError::NotSent(reason)), _)
                if !retrying =>
            {
                self.store.save(None)?;
                if let Some(directory) = &directory {
                    self.store.discard(directory);
                }
                Err(failed(reason))
            }
            (result, Some(mut identity)) => {
                let (world, name) = (identity.membership.world.to_string(), identity.membership.name.clone());
                if result.is_ok() {
                    identity.pending = false;
                    self.store.save(Some(&identity))?;
                }
                self.start(identity, keys)?;
                result.map_err(failed)?;
                // Check the inviter-signed kind from the actual member table,
                // not just what the hub handed this device.
                let session = self.connected()?;
                session.check_identity().await?;
                Ok((world, name))
            }
            (result, None) => Err(failed(result.err().map(|error| error.to_string()).unwrap_or_default())),
        }
    }

    /// Revokes this device directly at the hub, even when no machines are
    /// online. An offline failure keeps the identity for the next attempt.
    pub async fn leave(self: &Arc<Self>) -> Result<(), Error> {
        let device = self.clone();
        tokio::spawn(async move {
            let _changing = device.lifecycle.lock().await;
            device.restore()?;
            let session = device.connected()?;
            if !matches!(session.link.status(), Status::Stopped { .. }) {
                session.link.revoke(&session.link.membership().name, "left").await.map_err(failed)?;
            }
            device.store.save(None)?;
            session.link.stop();
            *device.session.write().expect("device session") = None;
            Ok(())
        })
        .await
        .map_err(failed)?
    }

    fn start(&self, identity: Identity, keys: Keys) -> io::Result<()> {
        let start = if self.managed { Link::start_managed } else { Link::start };
        let public = keys.sign.public();
        let handler = Arc::new(Handler { _store: self.store.clone() });
        let link = Arc::new(start(self.store.net(&identity), identity.membership, keys, handler)?);
        *self.session.write().expect("device session") = Some(Arc::new(Session { link, key: public }));
        Ok(())
    }

    /// Called under the lifecycle lock after a previous link startup failed.
    /// Admission on disk remains authoritative even when opening its local
    /// counter or OS notifications failed in this process.
    fn restore(&self) -> io::Result<()> {
        if self.session.read().expect("device session").is_none()
            && let Some(identity) = self.store.load()?
        {
            let keys = self.store.keys(&identity)?;
            self.start(identity, keys)?;
        }
        Ok(())
    }

    fn connected(&self) -> Result<Arc<Session>, Error> {
        self.session.read().expect("device session").clone().ok_or_else(|| failed("this device has not joined a World"))
    }
}

impl Session {
    async fn check_identity(&self) -> Result<(), Error> {
        self.link.refresh().await.map_err(failed)?;
        let membership = self.link.membership();
        let (table, _) = self.link.members();
        let member =
            table.get(&membership.name).ok_or_else(|| failed("the World does not trust this device's membership"))?;
        if member.kind != Kind::Device || member.key != self.key {
            return Err(failed("this identity was admitted as a machine, not a device"));
        }
        Ok(())
    }
}

pub(super) fn link_state(link: &Link) -> LinkState {
    match link.status() {
        Status::Connecting { since, last_error } => LinkState::Connecting { since, last_error },
        Status::Online { since, address, route } => LinkState::Online { since, hub: address.to_string(), route },
        Status::Stopped { reason } => LinkState::Stopped { reason },
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
}

impl fastctx_net::link::Node for Handler {
    fn answering(&self, _: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        Err(Failure::new("this member is a device; nothing runs on it"))
    }

    fn call(&self, _: MachineName, _: RemoteCall, _: Option<fastctx_net::data::Stream>) -> BoxFuture<RemoteResult> {
        Box::pin(async { Err(Failure::new("this member is a device; it accepts no incoming calls")) })
    }

    fn letter(
        &self,
        _: MachineName,
        _: fastctx_net::message::LetterId,
        _: Letter,
    ) -> BoxFuture<io::Result<Result<(), String>>> {
        Box::pin(async { Ok(Err("a device receives no letters".to_string())) })
    }

    fn ledger_changed(&self, _: Option<fastctx_net::ledger::TownId>) {}

    fn log(&self, line: String) {
        eprintln!("World device: {line}");
    }
}
