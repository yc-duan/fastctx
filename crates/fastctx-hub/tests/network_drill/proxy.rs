//! A machine's line to the hub, as the drill makes it fail. It passes bytes
//! both ways between the machine and the hub. Cut, it drops every
//! connection on it at once, as a network change does; frozen, it passes
//! nothing and closes nothing, as a path gone dead does, either way or one
//! way only; limited, each way of each connection passes only so many
//! bytes a second.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

pub struct Line {
    /// Where the machine reaches the hub through this line.
    pub address: SocketAddr,
    cut: watch::Sender<u64>,
    /// Whether what goes to the hub passes nothing, and what comes from it.
    to_hub: Arc<AtomicBool>,
    to_machine: Arc<AtomicBool>,
    rate: Arc<AtomicU64>,
}

impl Line {
    /// A line to the hub at `hub`.
    pub async fn open(hub: SocketAddr) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let (cut, cutting) = watch::channel(0);
        let line = Arc::new(Self {
            address: listener.local_addr()?,
            cut,
            to_hub: Arc::default(),
            to_machine: Arc::default(),
            rate: Arc::default(),
        });
        tokio::spawn(pass(listener, hub, cutting, line.to_hub.clone(), line.to_machine.clone(), line.rate.clone()));
        Ok(line)
    }

    /// Drops every connection open on it now.
    pub fn cut(&self) {
        self.cut.send_modify(|cuts| *cuts += 1);
    }

    /// Passes nothing either way, or everything again.
    pub fn freeze(&self, frozen: bool) {
        self.to_hub.store(frozen, Ordering::SeqCst);
        self.to_machine.store(frozen, Ordering::SeqCst);
    }

    /// Passes nothing from the hub to the machine, or everything again;
    /// what the machine sends still goes.
    pub fn freeze_to_machine(&self, frozen: bool) {
        self.to_machine.store(frozen, Ordering::SeqCst);
    }

    /// Passes at most `bytes` a second each way of each connection; none,
    /// as fast as it goes.
    pub fn limit(&self, bytes: u64) {
        self.rate.store(bytes, Ordering::SeqCst);
    }
}

async fn pass(
    listener: TcpListener,
    hub: SocketAddr,
    cutting: watch::Receiver<u64>,
    to_hub: Arc<AtomicBool>,
    to_machine: Arc<AtomicBool>,
    rate: Arc<AtomicU64>,
) {
    loop {
        let Ok((inbound, _)) = listener.accept().await else { return };
        let mut cutting = cutting.clone();
        cutting.mark_unchanged();
        let (to_hub, to_machine, rate) = (to_hub.clone(), to_machine.clone(), rate.clone());
        tokio::spawn(async move {
            let Ok(outbound) = TcpStream::connect(hub).await else { return };
            let _ = (inbound.set_nodelay(true), outbound.set_nodelay(true));
            let (from_machine, machine_end) = inbound.into_split();
            let (from_hub, hub_end) = outbound.into_split();
            let cut = async {
                // The line itself outlives every connection on it.
                if cutting.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = pipe(from_machine, hub_end, &to_hub, &rate) => {}
                () = pipe(from_hub, machine_end, &to_machine, &rate) => {}
                () = cut => {}
            }
        });
    }
}

async fn pipe(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, frozen: &AtomicBool, rate: &AtomicU64) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        thaw(frozen).await;
        let limit = rate.load(Ordering::SeqCst);
        // A tenth of a second's worth at a time, so a limited line passes
        // its bytes evenly.
        let want =
            if limit == 0 { buffer.len() } else { usize::try_from(limit / 10).unwrap_or(1).clamp(1, buffer.len()) };
        let Ok(read) = from.read(&mut buffer[..want]).await else { return };
        if read == 0 {
            return;
        }
        // What came in as the line froze waits with it.
        thaw(frozen).await;
        if to.write_all(&buffer[..read]).await.is_err() {
            return;
        }
        if limit > 0 {
            tokio::time::sleep(Duration::from_millis(read as u64 * 1000 / limit)).await;
        }
    }
}

async fn thaw(frozen: &AtomicBool) {
    while frozen.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
