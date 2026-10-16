//! How recently a connection's bytes moved, each way. A connection is
//! taken for gone once nothing moved on it for a silence (`world-network.md`,
//! 断开不是结束): a message still coming in, or still going out, shows the
//! line moves as much as one that is whole, so a slow line carrying a large
//! message is not taken for a dead one, and a dead one still ends within a
//! silence.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;

use crate::wire::SILENCE;

/// The moment pulses are counted from.
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now() -> u64 {
    u64::try_from(START.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// When a connection's bytes last moved, each way.
pub struct Pulse {
    read: AtomicU64,
    written: AtomicU64,
}

impl Pulse {
    fn new() -> Arc<Self> {
        let at = now();
        Arc::new(Self { read: AtomicU64::new(at), written: AtomicU64::new(at) })
    }

    /// When bytes last came in.
    pub fn read(&self) -> Instant {
        *START + Duration::from_millis(self.read.load(Ordering::Relaxed))
    }

    /// When bytes last went out.
    pub fn written(&self) -> Instant {
        *START + Duration::from_millis(self.written.load(Ordering::Relaxed))
    }

    /// When bytes last moved either way.
    pub fn moved(&self) -> Instant {
        self.read().max(self.written())
    }

    /// Waits for `work`, a write on this connection, for as long as bytes
    /// keep moving on it: past a silence with none moving either way, it
    /// gives up. Bytes coming in count too, as the system takes what is
    /// written in bursts, and says it took them only after a while: the
    /// other end, there and sending, is reading too, or what waits for it
    /// piles up past its bound (`crate::queue`).
    pub async fn writing<F: Future>(&self, work: F) -> Result<F::Output, Quiet> {
        let began = Instant::now();
        tokio::pin!(work);
        loop {
            let until = self.moved().max(began) + SILENCE;
            tokio::select! {
                done = &mut work => return Ok(done),
                () = tokio::time::sleep_until(until) => {
                    if self.moved().max(began) + SILENCE <= Instant::now() {
                        return Err(Quiet);
                    }
                }
            }
        }
    }

    /// Whether bytes came in since `heard`, the last whole message: then the
    /// moment they came, which the silence counts from instead.
    pub fn since(&self, heard: Instant) -> Option<Instant> {
        let read = self.read();
        (read > heard).then_some(read)
    }
}

/// Nothing moved on a connection for a silence.
#[derive(Debug)]
pub struct Quiet;

impl std::fmt::Display for Quiet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the other end took nothing for {} s", SILENCE.as_secs())
    }
}

/// A connection whose reads and writes keep its pulse.
pub struct Pulsing<S> {
    inner: S,
    pulse: Arc<Pulse>,
}

impl<S> Pulsing<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, pulse: Pulse::new() }
    }

    pub fn pulse(&self) -> Arc<Pulse> {
        self.pulse.clone()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Pulsing<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let read = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(read, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            this.pulse.read.store(now(), Ordering::Relaxed);
        }
        read
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Pulsing<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.inner).poll_write(cx, buf);
        if matches!(written, Poll::Ready(Ok(count)) if count > 0) {
            this.pulse.written.store(now(), Ordering::Relaxed);
        }
        written
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
