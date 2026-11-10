use super::{Encoded, INFLIGHT_PACKETS, Part, QUANTUM, SUBSCRIPTIONS, VERSION};
use crate::data::WriteHalf;
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, oneshot};

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

/// Each subscription has at most one packet in flight. Ready subscriptions take
/// turns by byte quantum, so a long history cannot monopolize their shared line.
pub async fn write(mut stream: WriteHalf, mut incoming: mpsc::Receiver<Output>) -> Result<(), String> {
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
        let total = pending.encoded.signed.data.0.len();
        if pending.cancelled.is_some() {
            if pending.offset != 0 {
                stream
                    .say(&Part {
                        watch_mux: VERSION,
                        subscription: id,
                        number: pending.encoded.number,
                        offset: pending.offset,
                        total,
                        bytes: 0,
                        signature: pending.encoded.signed.sig,
                        discard: true,
                    })
                    .await?;
            }
            let pending = packets.remove(&id).expect("cancelled packet");
            let _ = pending.cancelled.expect("cancellation waiter").send(());
            continue;
        }
        let bytes = (total - pending.offset).min(QUANTUM);
        stream
            .say(&Part {
                watch_mux: VERSION,
                subscription: id,
                number: pending.encoded.number,
                offset: pending.offset,
                total,
                bytes,
                signature: pending.encoded.signed.sig,
                discard: false,
            })
            .await?;
        stream.send(&pending.encoded.signed.data.0[pending.offset..pending.offset + bytes]).await?;
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

fn accept(output: Output, packets: &mut HashMap<u64, Pending>, ready: &mut VecDeque<u64>) -> Result<(), String> {
    match output {
        Output::Packet { encoded, sent } => {
            let id = encoded.subscription;
            if packets.contains_key(&id) || packets.len() >= INFLIGHT_PACKETS {
                return Err("too many packets waiting on the multiplexed watch".into());
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
