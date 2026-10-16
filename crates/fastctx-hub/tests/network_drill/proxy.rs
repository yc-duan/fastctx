//! A faulted line owns its listener and every connection until checked shutdown.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

pub struct Line {
    pub address: SocketAddr,
    cut: watch::Sender<u64>,
    stopping: watch::Sender<bool>,
    to_hub: Arc<AtomicBool>,
    to_machine: Arc<AtomicBool>,
    rate: Arc<AtomicU64>,
    blocked: Arc<AtomicU64>,
    task: Mutex<Option<JoinHandle<anyhow::Result<()>>>>,
}

impl Line {
    pub async fn open(hub: SocketAddr) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let (cut, cutting) = watch::channel(0);
        let (stopping, stopped) = watch::channel(false);
        let line = Arc::new(Self {
            address: listener.local_addr()?,
            cut,
            stopping,
            to_hub: Arc::default(),
            to_machine: Arc::default(),
            rate: Arc::default(),
            blocked: Arc::default(),
            task: Mutex::new(None),
        });
        let fault = Fault {
            to_hub: line.to_hub.clone(),
            to_machine: line.to_machine.clone(),
            rate: line.rate.clone(),
            blocked: line.blocked.clone(),
        };
        *line.task.lock().expect("never poisoned") = Some(tokio::spawn(pass(listener, hub, cutting, stopped, fault)));
        Ok(line)
    }

    pub fn cut(&self) {
        self.cut.send_modify(|cuts| *cuts += 1);
    }

    pub fn freeze(&self, frozen: bool) {
        self.to_hub.store(frozen, Ordering::SeqCst);
        self.to_machine.store(frozen, Ordering::SeqCst);
    }

    pub fn freeze_to_machine(&self, frozen: bool) {
        self.to_machine.store(frozen, Ordering::SeqCst);
    }
    pub fn limit(&self, bytes: u64) {
        self.rate.store(bytes, Ordering::SeqCst);
    }
    pub fn blocked(&self) -> u64 {
        self.blocked.load(Ordering::SeqCst)
    }

    pub async fn end(&self) -> anyhow::Result<()> {
        self.stopping.send_replace(true);
        self.cut();
        let task = self.task.lock().expect("never poisoned").take();
        if let Some(mut task) = task {
            match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
                Ok(result) => result??,
                Err(error) => {
                    task.abort();
                    let _ = task.await;
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
        if let Some(task) = self.task.lock().expect("never poisoned").take() {
            task.abort();
        }
    }
}

#[derive(Clone)]
struct Fault {
    to_hub: Arc<AtomicBool>,
    to_machine: Arc<AtomicBool>,
    rate: Arc<AtomicU64>,
    blocked: Arc<AtomicU64>,
}

async fn pass(
    listener: TcpListener,
    hub: SocketAddr,
    cutting: watch::Receiver<u64>,
    mut stopped: watch::Receiver<bool>,
    fault: Fault,
) -> anyhow::Result<()> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            ended = connections.join_next(), if !connections.is_empty() => { ended.expect("one connection")?; }
            accepted = listener.accept() => {
                let (inbound, _) = accepted?;
                let mut cutting = cutting.clone();
                cutting.mark_unchanged();
                let fault = fault.clone();
                connections.spawn(async move {
                    let connect = async {
                        let Ok(outbound) = TcpStream::connect(hub).await else { return };
                        let _ = (inbound.set_nodelay(true), outbound.set_nodelay(true));
                        let (from_machine, machine_end) = inbound.into_split();
                        let (from_hub, hub_end) = outbound.into_split();
                        tokio::select! {
                            () = pipe(from_machine, hub_end, &fault.to_hub, &fault.rate, &fault.blocked) => {}
                            () = pipe(from_hub, machine_end, &fault.to_machine, &fault.rate, &fault.blocked) => {}
                        }
                    };
                    tokio::select! { () = connect => {}, _ = cutting.changed() => {} }
                });
            }
        }
    }
    connections.abort_all();
    while let Some(ended) = connections.join_next().await {
        if let Err(error) = ended
            && !error.is_cancelled()
        {
            return Err(error.into());
        }
    }
    Ok(())
}

async fn pipe(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    frozen: &AtomicBool,
    rate: &AtomicU64,
    blocked: &AtomicU64,
) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let limit = rate.load(Ordering::SeqCst);
        let want =
            if limit == 0 { buffer.len() } else { usize::try_from(limit / 10).unwrap_or(1).clamp(1, buffer.len()) };
        let Ok(read) = from.read(&mut buffer[..want]).await else { return };
        if read == 0 {
            return;
        }
        if frozen.load(Ordering::SeqCst) {
            blocked.fetch_add(read as u64, Ordering::SeqCst);
        }
        while frozen.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            return;
        }
        if limit > 0 {
            tokio::time::sleep(Duration::from_millis(read as u64 * 1000 / limit)).await;
        }
    }
}
