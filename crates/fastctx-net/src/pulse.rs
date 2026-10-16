//! How a connection's bytes move, each way, and what the other end says it
//! took. A connection is taken for gone once nothing came from the other
//! end for a silence (`world-network.md`, 断开不是结束): a message still
//! coming in shows the other end is there as much as one come whole, so a
//! slow line carrying a large message is not taken for a dead one.
//!
//! Writing is told apart from the other end being there. Each end says, in
//! its heartbeats, how many bytes it has taken of what the other sent; a
//! write goes on as long as it goes out or that count grows, and is given
//! up once neither moved for a silence, however lively the other end's own
//! heartbeats: an end that is there and takes nothing is as gone as one that
//! says nothing. An end of an earlier release says nothing of what it took;
//! for it, bytes coming from it count as the write moving, for a while.

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

/// How long bytes coming from an end that says nothing of what it took keep
/// a write going: it may be reading, or it may only be saying it is there.
const UNTOLD_AT_MOST: Duration = Duration::from_secs(300);

fn now() -> u64 {
    u64::try_from(START.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn at(millis: u64) -> Instant {
    *START + Duration::from_millis(millis)
}

/// When a connection's bytes last moved, each way, and what the other end
/// said it took.
pub struct Pulse {
    read: AtomicU64,
    written: AtomicU64,
    /// How many bytes came in, in all.
    taken: AtomicU64,
    /// The most the other end said it took of what this end sent, and when
    /// that last grew; when it last said it at all, none before it did.
    told: AtomicU64,
    told_grew: AtomicU64,
    told_at: AtomicU64,
}

impl Pulse {
    fn new() -> Arc<Self> {
        let at = now();
        Arc::new(Self {
            read: AtomicU64::new(at),
            written: AtomicU64::new(at),
            taken: AtomicU64::new(0),
            told: AtomicU64::new(0),
            told_grew: AtomicU64::new(at),
            told_at: AtomicU64::new(0),
        })
    }

    /// When bytes last came in.
    pub fn read(&self) -> Instant {
        at(self.read.load(Ordering::Relaxed))
    }

    /// When bytes last went out.
    pub fn written(&self) -> Instant {
        at(self.written.load(Ordering::Relaxed))
    }

    /// How many bytes came in, in all: what this end tells the other it
    /// took.
    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// The other end says it took `bytes` of what this end sent.
    pub fn told(&self, bytes: u64) {
        let now = now();
        if self.told.fetch_max(bytes, Ordering::Relaxed) < bytes {
            self.told_grew.store(now, Ordering::Relaxed);
        }
        self.told_at.store(now.max(1), Ordering::Relaxed);
    }

    /// The last moment what this end writes was seen moving: bytes went
    /// out, or the other end said it took more. While the other end has not
    /// said for a silence what it took, bytes coming from it count too, up
    /// to a while past `began`.
    fn moving(&self, began: Instant) -> Instant {
        let moving = began.max(self.written()).max(at(self.told_grew.load(Ordering::Relaxed)));
        let told_at = self.told_at.load(Ordering::Relaxed);
        if told_at == 0 || at(told_at) + SILENCE < Instant::now() {
            moving.max(self.read().min(began + UNTOLD_AT_MOST))
        } else {
            moving
        }
    }

    /// Waits for `work`, a write on this connection, for as long as it
    /// moves (`moving`): past a silence without, it gives up. Bytes the
    /// system took are not enough alone, as it takes what is written in
    /// bursts and says it took them only after a while; what the other end
    /// says it took is.
    pub async fn writing<F: Future>(&self, work: F) -> Result<F::Output, Quiet> {
        let began = Instant::now();
        tokio::pin!(work);
        loop {
            let until = self.moving(began) + SILENCE;
            tokio::select! {
                done = &mut work => return Ok(done),
                () = tokio::time::sleep_until(until) => {
                    if self.moving(began) + SILENCE <= Instant::now() {
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

/// A WebSocket ping saying the other end's bytes this end took (`Pulse::told`).
pub fn ping(pulse: &Pulse) -> tokio_tungstenite::tungstenite::Message {
    tokio_tungstenite::tungstenite::Message::Ping(pulse.taken().to_be_bytes().to_vec().into())
}

/// What a ping from the other end says it took; none from an end of an
/// earlier release, whose pings say nothing.
pub fn said(ping: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(ping.try_into().ok()?))
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
        let came = buf.filled().len() - before;
        if matches!(read, Poll::Ready(Ok(()))) && came > 0 {
            this.pulse.read.store(now(), Ordering::Relaxed);
            this.pulse.taken.fetch_add(came as u64, Ordering::Relaxed);
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
