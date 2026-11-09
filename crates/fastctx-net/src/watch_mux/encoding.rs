use super::{MAX_PACKET, QUANTUM};
use crate::{
    link::Link,
    reassembly::Reservation,
    sign::{Domain, Signed},
};
use fastctx_types::watching::Packet;
use std::sync::Arc;

/// A measured packet without encoded storage or a process-wide reservation.
/// Wait for the consumer's acknowledgement window before encoding it.
pub struct Prepared {
    packet: Packet,
    bytes: usize,
}

impl Prepared {
    /// Uncompressed bytes charged to the consumer's acknowledgement window.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Encodes and reserves storage for a packet whose consumer window is ready.
    pub async fn encode(self, link: Arc<Link>) -> Result<Encoded, String> {
        fastctx_platform::task::spawn_blocking(move || encode_packet(&link, self))
            .await
            .map_err(|e| format!("encoding the watch packet: {e}"))?
    }
}

pub struct Encoded {
    pub(super) subscription: u64,
    pub(super) number: u64,
    pub(super) signed: Signed,
    _memory: Reservation,
    bytes: usize,
}

impl Encoded {
    /// Uncompressed bytes charged to the consumer's acknowledgement window.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Counts a packet in a blocking worker without reserving encoding storage.
pub async fn prepare(packet: Packet) -> Result<Prepared, String> {
    fastctx_platform::task::spawn_blocking(move || prepare_packet(packet))
        .await
        .map_err(|e| format!("measuring the watch packet: {e}"))?
}

pub async fn encode(link: Arc<Link>, packet: Packet) -> Result<Encoded, String> {
    fastctx_platform::task::spawn_blocking(move || encode_packet(&link, prepare_packet(packet)?))
        .await
        .map_err(|e| format!("encoding the watch packet: {e}"))?
}

fn prepare_packet(packet: Packet) -> Result<Prepared, String> {
    let bytes = crate::watch_queue::size(&packet);
    if bytes > MAX_PACKET {
        return Err("the watch packet exceeds 64 MiB; its history must be sent in smaller pieces".into());
    }
    Ok(Prepared { packet, bytes })
}

fn encode_packet(link: &Link, prepared: Prepared) -> Result<Encoded, String> {
    let Prepared { packet, bytes } = prepared;
    let memory = Reservation::new(bytes.saturating_mul(2).saturating_add(2 * QUANTUM))?;
    let signed = link.sign_bytes(Domain::Event, crate::history::pack(&packet));
    if signed.data.0.is_empty() || signed.data.0.len() > MAX_PACKET {
        return Err("the encoded watch packet exceeds its bound".into());
    }
    Ok(Encoded { subscription: packet.subscription, number: packet.number, signed, _memory: memory, bytes })
}
