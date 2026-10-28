use super::{INFLIGHT_PACKETS, MAX_PACKET, Part, QUANTUM, VERSION};
use crate::{
    data::ReadHalf,
    reassembly::{Buffer, Reservation},
    sign::{Bytes, Signed},
};
use std::collections::HashMap;
use std::ops::Deref;

pub struct Received {
    signed: Signed,
    _memory: Reservation,
    pub subscription: u64,
    pub number: u64,
}
impl Deref for Received {
    type Target = Signed;
    fn deref(&self) -> &Signed {
        &self.signed
    }
}
struct Assembly {
    number: u64,
    total: usize,
    signature: crate::sign::Signature,
    bytes: Buffer,
}

#[derive(Default)]
pub struct Reader {
    pending: HashMap<u64, Assembly>,
}
impl Reader {
    /// Consumes a whole chunk before returning. The caller must keep this read in
    /// its own task; cancelling midway through a chunk closes the whole stream.
    pub async fn next(
        &mut self,
        stream: &mut ReadHalf,
        first: Option<serde_json::Value>,
    ) -> Result<Option<Received>, String> {
        let mut first = first;
        loop {
            let part: Part = match first.take() {
                Some(value) => {
                    serde_json::from_value(value).map_err(|e| format!("the multiplexed watch header: {e}"))?
                }
                None => match stream.hear_or_end().await? {
                    Some(part) => part,
                    None => return Ok(None),
                },
            };
            if part.watch_mux == VERSION && part.discard && part.bytes == 0 {
                if let Some(assembly) = self.pending.remove(&part.subscription)
                    && (assembly.number != part.number
                        || assembly.signature != part.signature
                        || assembly.total != part.total
                        || assembly.bytes.len() != part.offset)
                {
                    return Err("the discarded watch packet changed identity".into());
                }
                continue;
            }
            if part.discard
                || part.watch_mux != VERSION
                || part.bytes == 0
                || part.bytes > QUANTUM
                || part.total > MAX_PACKET
                || part.offset.checked_add(part.bytes).is_none_or(|end| end > part.total)
            {
                return Err("invalid multiplexed watch chunk bounds".into());
            }
            if !self.pending.contains_key(&part.subscription) {
                if part.offset != 0 || self.pending.len() >= INFLIGHT_PACKETS {
                    return Err("the multiplexed watch has an unexpected or excessive partial packet".into());
                }
                self.pending.insert(
                    part.subscription,
                    Assembly {
                        number: part.number,
                        total: part.total,
                        signature: part.signature,
                        bytes: Buffer::new(part.total, part.total + 2 * QUANTUM)?,
                    },
                );
            }
            let assembly = self.pending.get_mut(&part.subscription).expect("inserted assembly");
            if assembly.number != part.number
                || assembly.total != part.total
                || assembly.signature != part.signature
                || assembly.bytes.len() != part.offset
            {
                return Err("multiplexed watch chunks changed identity or arrived out of order".into());
            }
            stream.take(part.bytes as u64, |bytes| assembly.bytes.extend(bytes)).await?;
            if assembly.bytes.len() == assembly.total {
                let assembly = self.pending.remove(&part.subscription).expect("complete assembly");
                let signed = Signed { data: Bytes(assembly.bytes.to_vec()), sig: assembly.signature };
                return Ok(Some(Received {
                    signed,
                    _memory: assembly.bytes.into_reservation(),
                    subscription: part.subscription,
                    number: part.number,
                }));
            }
        }
    }
}
