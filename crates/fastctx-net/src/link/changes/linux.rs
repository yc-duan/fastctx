use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use tokio::io::unix::AsyncFd;

use crate::wire::HEARTBEAT;

#[path = "resume.rs"]
mod resume;

pub(crate) struct Changes {
    network: AsyncFd<OwnedFd>,
    timer: AsyncFd<OwnedFd>,
    sampled: fastctx_platform::awake::Sample,
    resume: resume::Resume,
    log: std::sync::Arc<dyn super::super::Node>,
}

impl Changes {
    pub fn new(log: std::sync::Arc<dyn super::super::Node>) -> io::Result<Self> {
        // SAFETY: no pointers; the returned descriptor is owned below.
        let network = owned(unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        })?;
        // SAFETY: zero is valid for sockaddr_nl, including its padding.
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as _;
        address.nl_groups = (libc::RTMGRP_LINK
            | libc::RTMGRP_IPV4_IFADDR
            | libc::RTMGRP_IPV6_IFADDR
            | libc::RTMGRP_IPV4_ROUTE
            | libc::RTMGRP_IPV6_ROUTE) as _;
        // SAFETY: a live socket and an initialized address of the stated size.
        if unsafe { libc::bind(network.as_raw_fd(), (&raw const address).cast(), size_of_val(&address) as _) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // BOOTTIME timers expire during sleep and are readable on resume;
        // they do not wake the machine. A short sleep not crossing this
        // deadline is detected within one heartbeat. A kernel before 3.15
        // has no BOOTTIME timer and refuses it as invalid: a MONOTONIC one
        // stands still while the machine sleeps, so a resume is seen at the
        // first heartbeat after it, by the clocks it compares.
        // SAFETY: no pointers; the returned descriptor is owned below.
        let flags = libc::TFD_NONBLOCK | libc::TFD_CLOEXEC;
        let timer = match owned(unsafe { libc::timerfd_create(libc::CLOCK_BOOTTIME, flags) }) {
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
                // SAFETY: as above.
                owned(unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, flags) })?
            }
            timer => timer?,
        };
        let period = libc::timespec { tv_sec: HEARTBEAT.as_secs() as _, tv_nsec: 0 };
        let spec = libc::itimerspec { it_interval: period, it_value: period };
        // SAFETY: a timer descriptor and initialized specification.
        if unsafe { libc::timerfd_settime(timer.as_raw_fd(), 0, &spec, std::ptr::null_mut()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            network: AsyncFd::new(network)?,
            timer: AsyncFd::new(timer)?,
            sampled: fastctx_platform::awake::Sample::now()?,
            resume: resume::Resume::new(),
            log,
        })
    }

    pub async fn changed(&mut self) -> io::Result<&'static str> {
        loop {
            tokio::select! {
                resumed = self.resume.recv() => {
                    match resumed {
                        Ok(()) => {
                            self.sampled = fastctx_platform::awake::Sample::now()?;
                            return Ok("the machine resumed (logind)");
                        },
                        Err(error) => self.log.log(error),
                    }
                }
                result = readable(&self.network, true) => {
                    result?;
                    return Ok("network interfaces, addresses or routes changed");
                }
                result = readable(&self.timer, false) => {
                    result?;
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

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the new descriptor is transferred once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

async fn readable(fd: &AsyncFd<OwnedFd>, network: bool) -> io::Result<()> {
    loop {
        let mut ready = fd.readable().await?;
        let read = ready.try_io(|fd| {
            // Only readiness matters, not individual route records. A
            // zero-length datagram receive consumes one complete datagram
            // without imposing a limit on the number of interfaces.
            let mut expirations = 0_u64;
            // SAFETY: a live descriptor; read has space for its u64 result.
            let result = unsafe {
                if network {
                    libc::recv(fd.get_ref().as_raw_fd(), std::ptr::null_mut(), 0, libc::MSG_TRUNC)
                } else {
                    libc::read(fd.get_ref().as_raw_fd(), (&raw mut expirations).cast(), size_of::<u64>())
                }
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                // ENOBUFS means notifications were lost. Re-dialing reads
                // the current state, so it also reconciles a lost event.
                if network && error.raw_os_error() == Some(libc::ENOBUFS) {
                    return Ok(());
                }
                return Err(error);
            }
            Ok(())
        });
        if let Ok(result) = read {
            return result;
        }
    }
}
