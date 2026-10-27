use super::{INFLIGHT_PACKETS, MAX_PACKET, Part, QUANTUM, SUBSCRIPTIONS, TIMEOUT};
use crate::reassembly::Reservation;
use fastctx_platform::ipc::FrameWriter;
use fastctx_types::watching::local::Packet;
use std::{
    collections::{HashMap, VecDeque},
    io,
};
use tokio::sync::{mpsc, oneshot};

pub struct Encoded {
    subscription: u64,
    number: u64,
    bytes: Vec<u8>,
    _memory: Reservation,
}
pub enum Output {
    Packet { encoded: Encoded, sent: oneshot::Sender<()> },
    Cancel { subscription: u64, cancelled: oneshot::Sender<()> },
}
struct Pending {
    encoded: Encoded,
    offset: usize,
    sent: oneshot::Sender<()>,
    cancelled: Option<oneshot::Sender<()>>,
}

pub async fn send(output: &mpsc::Sender<Output>, packet: Packet) -> io::Result<()> {
    let size = crate::watch_queue::size(&packet);
    if size > MAX_PACKET {
        return Err(io::Error::other("the local watch packet exceeds 64 MiB"));
    }
    let memory = Reservation::new(size * 2 + QUANTUM).map_err(io::Error::other)?;
    let encode = move || {
        Ok::<_, io::Error>(Encoded {
            subscription: packet.subscription,
            number: packet.number,
            bytes: serde_json::to_vec(&packet)?,
            _memory: memory,
        })
    };
    let encoded = if size <= QUANTUM {
        encode()?
    } else {
        tokio::task::spawn_blocking(encode).await.map_err(io::Error::other)??
    };
    let (sent, written) = oneshot::channel();
    tokio::time::timeout(TIMEOUT, async {
        output
            .send(Output::Packet { encoded, sent })
            .await
            .map_err(|_| io::Error::other("the local watch writer closed"))?;
        written.await.map_err(io::Error::other)
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn cancel(output: &mpsc::Sender<Output>, subscription: u64) -> io::Result<()> {
    let (cancelled, received) = oneshot::channel();
    tokio::time::timeout(TIMEOUT, async {
        output
            .send(Output::Cancel { subscription, cancelled })
            .await
            .map_err(|_| io::Error::other("the local watch writer closed"))?;
        received.await.map_err(io::Error::other)
    })
    .await
    .map_err(io::Error::other)?
}

/// One ready packet per subscription, rotating after each bounded byte chunk.
pub async fn write(mut stream: FrameWriter, mut incoming: mpsc::Receiver<Output>) -> io::Result<()> {
    let mut packets = HashMap::<u64, Pending>::new();
    let mut ready = VecDeque::new();
    let mut open = true;
    loop {
        if ready.is_empty() {
            match incoming.recv().await {
                Some(output) => accept(output, &mut packets, &mut ready)?,
                None => return Ok(()),
            }
        }
        for _ in 0..SUBSCRIPTIONS + 1 {
            match incoming.try_recv() {
                Ok(output) => accept(output, &mut packets, &mut ready)?,
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        let Some(id) = ready.pop_front() else {
            if !open {
                return Ok(());
            }
            continue;
        };
        let pending = packets.get_mut(&id).expect("ready packet");
        let total = pending.encoded.bytes.len();
        if pending.cancelled.is_some() {
            if pending.offset != 0 {
                let part = Part {
                    subscription: id,
                    number: pending.encoded.number,
                    offset: pending.offset,
                    total,
                    bytes: 0,
                    discard: true,
                };
                tokio::time::timeout(TIMEOUT, stream.send(&serde_json::to_vec(&part)?))
                    .await
                    .map_err(io::Error::other)??;
            }
            let pending = packets.remove(&id).expect("cancelled packet");
            let _ = pending.cancelled.expect("cancellation waiter").send(());
            continue;
        }
        let bytes = (total - pending.offset).min(QUANTUM);
        let part = Part {
            subscription: id,
            number: pending.encoded.number,
            offset: pending.offset,
            total,
            bytes,
            discard: false,
        };
        tokio::time::timeout(TIMEOUT, async {
            stream.send(&serde_json::to_vec(&part)?).await?;
            stream.send(&pending.encoded.bytes[pending.offset..pending.offset + bytes]).await
        })
        .await
        .map_err(io::Error::other)??;
        pending.offset += bytes;
        if pending.offset == total {
            let pending = packets.remove(&id).expect("sent packet");
            let _ = pending.sent.send(());
        } else {
            ready.push_back(id);
        }
        if !open && ready.is_empty() {
            return Ok(());
        }
    }
}

fn accept(output: Output, packets: &mut HashMap<u64, Pending>, ready: &mut VecDeque<u64>) -> io::Result<()> {
    match output {
        Output::Packet { encoded, sent } => {
            let id = encoded.subscription;
            if packets.contains_key(&id) || packets.len() >= INFLIGHT_PACKETS {
                return Err(io::Error::other("too many packets waiting on the local shared watch"));
            }
            packets.insert(id, Pending { encoded, offset: 0, sent, cancelled: None });
            ready.push_back(id);
        }
        Output::Cancel { subscription, cancelled } => {
            if let Some(packet) = packets.get_mut(&subscription) {
                packet.cancelled = Some(cancelled);
            } else {
                let _ = cancelled.send(());
            }
        }
    }
    Ok(())
}
