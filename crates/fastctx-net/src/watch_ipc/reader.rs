use super::{HEADER, INFLIGHT_PACKETS, MAX_PACKET, Part, QUANTUM, TIMEOUT};
use crate::reassembly::{Buffer, Reservation};
use fastctx_platform::ipc::FrameReader;
use fastctx_types::{ipc::NodeFrame, watching::local::Packet};
use std::{collections::HashMap, io};

pub struct Received {
    pub packet: Packet,
    pub memory: Reservation,
}
struct Assembly {
    number: u64,
    total: usize,
    bytes: Buffer,
    began: tokio::time::Instant,
}

#[derive(Default)]
pub struct Reader {
    pending: HashMap<u64, Assembly>,
}
impl Reader {
    /// Reads whole chunks in one dedicated task. Dropping this future must also
    /// close the connection because its header may already have been consumed.
    pub async fn next(&mut self, stream: &mut FrameReader) -> io::Result<Option<Received>> {
        loop {
            let head = if let Some(deadline) = self.pending.values().map(|a| a.began + TIMEOUT).min() {
                tokio::time::timeout_at(deadline, stream.recv_up_to(HEADER)).await.map_err(io::Error::other)??
            } else {
                stream.recv_up_to(HEADER).await?
            };
            let Some(head) = head else {
                if !self.pending.is_empty() {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                return Ok(None);
            };
            let part: Part = serde_json::from_slice(&head).map_err(|error| {
                if let Ok(NodeFrame::Reply { reply: Err(failure), .. }) = serde_json::from_slice(&head) {
                    io::Error::other(failure.message)
                } else {
                    io::Error::other(error)
                }
            })?;
            if part.discard && part.bytes == 0 {
                if let Some(assembly) = self.pending.remove(&part.subscription)
                    && (assembly.number != part.number
                        || assembly.total != part.total
                        || assembly.bytes.len() != part.offset)
                {
                    return Err(io::Error::other("the discarded local watch packet changed identity"));
                }
                continue;
            }
            if part.discard
                || part.bytes == 0
                || part.bytes > QUANTUM
                || part.total > MAX_PACKET
                || part.offset.checked_add(part.bytes).is_none_or(|end| end > part.total)
            {
                return Err(io::Error::other("invalid local watch chunk bounds"));
            }
            if !self.pending.contains_key(&part.subscription) {
                if part.offset != 0 || self.pending.len() >= INFLIGHT_PACKETS {
                    return Err(io::Error::other("unexpected or excessive partial local watch packets"));
                }
                self.pending.insert(
                    part.subscription,
                    Assembly {
                        number: part.number,
                        total: part.total,
                        bytes: Buffer::new(part.total, part.total + 2 * QUANTUM).map_err(io::Error::other)?,
                        began: tokio::time::Instant::now(),
                    },
                );
            }
            let assembly = self.pending.get_mut(&part.subscription).expect("inserted assembly");
            if assembly.number != part.number || assembly.total != part.total || assembly.bytes.len() != part.offset {
                return Err(io::Error::other("local watch chunks changed identity or arrived out of order"));
            }
            let chunk = tokio::time::timeout(TIMEOUT, stream.recv_up_to(part.bytes))
                .await
                .map_err(io::Error::other)??
                .ok_or(io::ErrorKind::UnexpectedEof)?;
            if chunk.len() != part.bytes {
                return Err(io::Error::other("the local watch chunk is truncated"));
            }
            assembly.bytes.extend(&chunk).map_err(io::Error::other)?;
            if assembly.bytes.len() == assembly.total {
                let assembly = self.pending.remove(&part.subscription).expect("complete assembly");
                let packet: Packet = serde_json::from_slice(&assembly.bytes)?;
                if packet.subscription != part.subscription || packet.number != part.number {
                    return Err(io::Error::other("the local watch packet differs from its header"));
                }
                return Ok(Some(Received { packet, memory: assembly.bytes.into_reservation() }));
            }
        }
    }
}
