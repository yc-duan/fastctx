use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use tokio::io::unix::AsyncFd;

use crate::wire::HEARTBEAT;

/// macOS tells of network changes on a routing socket. Every neighbour the
/// machine meets also comes there, as a cloned or link-layer route, and is no
/// change of the machine's own network: only addresses, interfaces and the
/// routes the system was given count.
pub(crate) struct Changes {
    network: AsyncFd<OwnedFd>,
    timer: tokio::time::Interval,
    sampled: fastctx_platform::awake::Sample,
}

/// `net/route.h`: the message types and route flags read here.
const RTM_ADD: u8 = 0x1;
const RTM_DELETE: u8 = 0x2;
const RTM_CHANGE: u8 = 0x3;
const RTM_NEWADDR: u8 = 0xc;
const RTM_DELADDR: u8 = 0xd;
const RTM_IFINFO: u8 = 0xe;
const RTF_HOST: i32 = 0x4;
const RTF_LLINFO: i32 = 0x400;
const RTF_WASCLONED: i32 = 0x20000;

impl Changes {
    pub fn new(_log: std::sync::Arc<dyn super::super::Node>) -> io::Result<Self> {
        // SAFETY: no pointers; the returned descriptor is owned below.
        let fd = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, libc::AF_UNSPEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: ownership of the new descriptor is transferred once.
        let network = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: plain flag changes on a descriptor owned here.
        unsafe {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                || libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) == -1
            {
                return Err(io::Error::last_os_error());
            }
        }
        let mut timer = tokio::time::interval(HEARTBEAT);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self { network: AsyncFd::new(network)?, timer, sampled: fastctx_platform::awake::Sample::now()? })
    }

    pub async fn changed(&mut self) -> io::Result<&'static str> {
        loop {
            tokio::select! {
                changed = changed(&self.network) => {
                    if changed? {
                        return Ok("network interfaces, addresses or routes changed");
                    }
                }
                _ = self.timer.tick() => {
                    let now = fastctx_platform::awake::Sample::now()?;
                    let resumed = now.suspended_since(&self.sampled);
                    let delayed = now.awake.saturating_sub(self.sampled.awake) >= HEARTBEAT * 2;
                    self.sampled = now;
                    if resumed || delayed {
                        return Ok("the machine resumed or the node resumed execution");
                    }
                }
            }
        }
    }
}

/// Reads one routing message; whether it changed the machine's own network.
async fn changed(fd: &AsyncFd<OwnedFd>) -> io::Result<bool> {
    let mut buffer = [0u8; 2048];
    loop {
        let mut ready = fd.readable().await?;
        let read = ready.try_io(|fd| {
            // SAFETY: a live descriptor and a buffer of its own length.
            let n = unsafe { libc::recv(fd.get_ref().as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0) };
            if n < 0 {
                let error = io::Error::last_os_error();
                // Lost messages: dialing again reads the current state.
                if error.raw_os_error() == Some(libc::ENOBUFS) {
                    return Ok(true);
                }
                return Err(error);
            }
            Ok(counts(&buffer[..n as usize]))
        });
        if let Ok(result) = read {
            return result;
        }
    }
}

/// A routing message's header begins with its length, version and type; a
/// route's flags follow its interface index.
fn counts(message: &[u8]) -> bool {
    let Some(&kind) = message.get(3) else { return false };
    match kind {
        RTM_NEWADDR | RTM_DELADDR | RTM_IFINFO => true,
        RTM_ADD | RTM_DELETE | RTM_CHANGE => {
            let Some(flags) = message.get(8..12) else { return false };
            let flags = i32::from_ne_bytes([flags[0], flags[1], flags[2], flags[3]]);
            flags & (RTF_HOST | RTF_LLINFO | RTF_WASCLONED) == 0
        }
        _ => false,
    }
}
