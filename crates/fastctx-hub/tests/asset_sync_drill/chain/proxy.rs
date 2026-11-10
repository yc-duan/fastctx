//! Delays arrival timestamps in parallel; it does not sleep per emitted packet.
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
pub(super) struct Proxy {
    pub address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
    count: Arc<AtomicUsize>,
}
impl Proxy {
    pub async fn start(target: SocketAddr, delay: Duration) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let task = tokio::spawn(async move {
            while let Ok((incoming, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::AcqRel);
                tokio::spawn(async move {
                    let outgoing = TcpStream::connect(target).await?;
                    incoming.set_nodelay(true)?;
                    outgoing.set_nodelay(true)?;
                    let (a, b) = incoming.into_split();
                    let (c, d) = outgoing.into_split();
                    tokio::try_join!(pipe(a, d, delay), pipe(c, b, delay))?;
                    Ok::<_, std::io::Error>(())
                });
            }
        });
        Ok(Self { address, task, count })
    }
    pub fn connections(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn pipe(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
) -> std::io::Result<()> {
    let (send, mut receive) = tokio::sync::mpsc::channel(32);
    let read = async move {
        loop {
            let mut bytes = vec![0; 65536];
            let count = reader.read(&mut bytes).await?;
            if count == 0 {
                return Ok::<_, std::io::Error>(());
            }
            bytes.truncate(count);
            if send.send((tokio::time::Instant::now() + delay, bytes)).await.is_err() {
                return Ok(());
            }
        }
    };
    let write = async move {
        while let Some((due, bytes)) = receive.recv().await {
            tokio::time::sleep_until(due).await;
            writer.write_all(&bytes).await?;
        }
        writer.shutdown().await
    };
    tokio::try_join!(read, write)?;
    Ok(())
}
