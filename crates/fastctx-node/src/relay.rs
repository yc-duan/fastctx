//! This machine as a relay for the machines whose routes lead through it
//! (`world-network.md`, 一个中枢，一棵树).
//!
//! ```text
//! world/net/relay.json              where it listens, and where machines below dial it
//! world/net/relay/certificate.der   the certificate they pin, kept so it stays the same
//! world/net/relay/key.der
//! ```
//!
//! While the settings are there the relay runs with the node, and this
//! machine's record says where it is reached; its own link connects through
//! it first, so it and the machines below it speak without the root.
//!
//! A relay is let go only once this machine's own link has left it: the
//! answer to the call that changed it goes out on that link, which a relay
//! stopped at once would cut. So a new relay starts beside the old, and the
//! old stops when the link has moved off it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use fastctx_hub::relay::{Machine, Relay, Settings};
use fastctx_net::link::Link;
use fastctx_net::sign::Signature;
use fastctx_net::tls::HubCertificate;
use fastctx_net::tree::{Endpoint, RelayOffer, Upstream};
use fastctx_net::trust::Table;
use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::ipc::Failure;
use fastctx_types::world::RelaySetting;

use crate::Node;

/// The relay while it runs, and what this machine's record should say of it.
pub(crate) struct Running {
    relay: Relay,
    setting: RelaySetting,
    offer: RelayOffer,
    /// Where this machine's own link reaches it.
    local: Endpoint,
}

fn settings_file(node: &Node) -> PathBuf {
    node.world.net().join("relay.json")
}

fn certificate_dir(node: &Node) -> PathBuf {
    node.world.net().join("relay")
}

fn link_of(node: &Node) -> Result<Arc<Link>, Failure> {
    node.link().ok_or_else(|| Failure::new(format!("{} is in no World with other machines", node.machine())))
}

/// Starts the relay the settings on disk describe, if any.
pub async fn resume(node: &Arc<Node>) {
    let setting = match std::fs::read(settings_file(node)) {
        Ok(bytes) => match serde_json::from_slice::<RelaySetting>(&bytes) {
            Ok(setting) => setting,
            Err(error) => {
                node.log.line(format_args!("the relay settings cannot be read: {error}"));
                return;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            node.log.line(format_args!("reading the relay settings: {error}"));
            return;
        }
    };
    if let Err(failure) = start(node, setting).await {
        node.log.line(format_args!("starting the relay: {}", failure.message));
    }
}

/// Relays as `setting` says, or stops relaying; returns where machines
/// below dial it.
pub async fn set(node: &Arc<Node>, setting: Option<RelaySetting>) -> Result<Vec<String>, Failure> {
    let link = link_of(node)?;
    match setting {
        Some(setting) => {
            if setting.public.is_empty() {
                return Err(Failure::new("a relay needs at least one address the machines below it dial"));
            }
            let bytes = serde_json::to_vec_pretty(&setting).expect("settings serialize");
            ensure_private_dir(&node.world.net())
                .and_then(|()| publish(&settings_file(node), &bytes, Permissions::Private))
                .map_err(|error| Failure::new(format!("keeping the relay settings: {error}")))?;
            let offer = start(node, setting).await?;
            link.offer_relay(Some(offer.clone())).await.map_err(|error| {
                Failure::new(format!("the relay runs, but saying so in this machine's record failed: {error}"))
            })?;
            Ok(offer.addresses.iter().map(ToString::to_string).collect())
        }
        None => {
            match std::fs::remove_file(settings_file(node)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Failure::new(format!("removing the relay settings: {error}")));
                }
                _ => {}
            }
            let stopping = node.relay.lock().await.take();
            link.offer_relay(None).await.map_err(|error| {
                Failure::new(format!("the relay stops, but saying so in this machine's record failed: {error}"))
            })?;
            if let Some(running) = stopping {
                link.relay_gone(&running.local);
                retire(running);
            }
            Ok(Vec::new())
        }
    }
}

/// Starts the relay `setting` describes in place of the one running, if
/// any; one listening where the running one does is only told its new
/// addresses.
async fn start(node: &Arc<Node>, setting: RelaySetting) -> Result<RelayOffer, Failure> {
    let link = link_of(node)?;
    let certificate = certificate(node).map_err(|error| Failure::new(format!("the relay's certificate: {error}")))?;
    let offer = RelayOffer { addresses: setting.public.clone(), cert: certificate.pin() };
    let mut running = node.relay.lock().await;
    if let Some(same) = running.as_mut().filter(|running| running.setting.listen == setting.listen) {
        same.setting = setting;
        same.offer = offer.clone();
        return Ok(offer);
    }
    let local = Endpoint {
        upstream: Upstream::Relay { machine: link.membership().name.clone() },
        addresses: vec![loopback(setting.listen)],
        cert: certificate.pin(),
    };
    let machine = Arc::new(Here { link: link.clone(), node: Arc::downgrade(node), local: local.clone() });
    let membership = link.membership();
    let relay = Relay::start(
        membership.name.clone(),
        membership.world.clone(),
        link.public(),
        machine,
        Settings { listen: setting.listen, certificate },
    )
    .await
    .map_err(Failure::new)?;
    let earlier = running.replace(Running { relay, setting, offer: offer.clone(), local });
    drop(running);
    if let Some(earlier) = earlier {
        link.relay_gone(&earlier.local);
        retire(earlier);
    }
    Ok(offer)
}

/// Lets `relay` go once this machine's own session is no longer attached
/// to it, and everything said on that session was passed on.
fn retire(relay: Running) {
    tokio::spawn(async move {
        relay.relay.released().await;
        drop(relay);
    });
}

/// The members changed: the relay checks its upstreams, and this machine's
/// record is brought to say what the relay is, when it says otherwise.
pub fn members_changed(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let Some(link) = node.link() else { return };
        let offer = {
            let running = node.relay.lock().await;
            if let Some(running) = running.as_ref() {
                running.relay.members_changed();
            }
            running.as_ref().map(|running| running.offer.clone())
        };
        let said = link.me().and_then(|me| me.relay);
        if said != offer
            && let Err(error) = link.offer_relay(offer).await
        {
            node.log.line(format_args!("saying in this machine's record whether it relays: {error}"));
        }
    });
}

/// The relay's certificate, made the first time.
fn certificate(node: &Node) -> std::io::Result<HubCertificate> {
    let dir = certificate_dir(node);
    let (certificate, key) = (dir.join("certificate.der"), dir.join("key.der"));
    match (std::fs::read(&certificate), std::fs::read(&key)) {
        (Ok(certificate), Ok(key)) => Ok(HubCertificate { certificate, key }),
        _ => {
            let made = HubCertificate::generate().map_err(std::io::Error::other)?;
            ensure_private_dir(&dir)?;
            publish(&key, &made.key, Permissions::Private)?;
            publish(&certificate, &made.certificate, Permissions::Private)?;
            Ok(made)
        }
    }
}

/// Where this machine reaches its own relay.
fn loopback(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}

/// The relay's view of this machine, through its link.
struct Here {
    link: Arc<Link>,
    node: Weak<Node>,
    local: Endpoint,
}

impl Machine for Here {
    fn table(&self) -> Arc<Table> {
        self.link.table()
    }

    fn upstreams(&self) -> Vec<Endpoint> {
        self.link.endpoints(&self.link.membership().name)
    }

    fn prove(&self, binding: &[u8; 32]) -> Signature {
        self.link.prove(binding)
    }

    fn refresh(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if let Err(error) = self.link.refresh().await {
                self.log(format!("the relay asking for the members: {error}"));
            }
        })
    }

    fn ready(&self, ready: bool) {
        if ready {
            self.link.relay_here(Some(self.local.clone()));
        } else {
            self.link.relay_gone(&self.local);
        }
    }

    fn log(&self, line: String) {
        if let Some(node) = self.node.upgrade() {
            node.log.line(line);
        }
    }
}
