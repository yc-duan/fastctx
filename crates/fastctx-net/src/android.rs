//! Android supplies Network handles and its per-destination proxy selection.
//! Neither interface names nor source addresses identify an Android Network.

use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::sync::Arc;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Network {
    pub handle: u64,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Proxy {
    Http { host: String, port: u16 },
    Direct,
    Unsupported { description: String },
}

#[derive(Deserialize)]
pub struct Choices {
    pub networks: Vec<Network>,
    pub proxies: Vec<Proxy>,
}

/// Installed by the application before it opens any device identity.
/// Proxy selection must honour the system's exclusion list and PAC result.
pub trait Platform: Send + Sync {
    fn choices(&self, target: SocketAddr) -> Result<Choices, String>;
}

/// The bridge last installed: a start that failed after installing it is
/// tried again with a new one.
static PLATFORM: std::sync::RwLock<Option<Arc<dyn Platform>>> = std::sync::RwLock::new(None);
static CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// Cancels every in-flight dial, including initial admission before a Link
/// exists. Only transport establishment is retried; sent calls are never replayed.
pub fn network_changed() {
    crate::dial::network_changed();
    CHANGES.send_modify(|generation| *generation = generation.wrapping_add(1));
}

pub(crate) fn changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

pub fn install(platform: Arc<dyn Platform>) {
    *PLATFORM.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(platform);
}

pub(crate) fn choices(target: SocketAddr) -> Result<Choices, String> {
    let platform = PLATFORM.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    platform.ok_or("the Android network bridge has not started")?.choices(target)
}

pub(crate) fn socket(network: &Network, target: SocketAddr) -> Result<tokio::net::TcpSocket, String> {
    let socket = if target.is_ipv6() { tokio::net::TcpSocket::new_v6() } else { tokio::net::TcpSocket::new_v4() }
        .map_err(|error| error.to_string())?;
    // API 23+: binding precedes connect and covers this socket only, never
    // the WebView or the application's other network users.
    #[link(name = "android")]
    unsafe extern "C" {
        fn android_setsocknetwork(network: u64, fd: i32) -> i32;
    }
    if unsafe { android_setsocknetwork(network.handle, socket.as_raw_fd()) } != 0 {
        return Err(format!("binding to {}: {}", network.name, std::io::Error::last_os_error()));
    }
    Ok(socket)
}
