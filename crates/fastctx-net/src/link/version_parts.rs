//! Version snapshots are assembled within one control connection. A newer
//! snapshot replaces any incomplete one; only a complete snapshot is published.
use fastctx_types::update::Network;

#[derive(Default)]
pub(super) struct Receiving {
    pending: Option<Pending>,
}
struct Pending {
    next: u32,
    bytes: usize,
    network: Network,
}

impl Receiving {
    pub(super) fn clear(&mut self) {
        self.pending = None;
    }

    pub(super) fn part(&mut self, index: u32, last: bool, part: Network) -> Result<Option<Network>, String> {
        let bytes = crate::wire::encode(&part).len();
        if index == 0 {
            self.pending = Some(Pending { next: 1, bytes, network: part });
        } else {
            let pending = self.pending.as_mut().ok_or("a version snapshot did not start at zero")?;
            if pending.next != index {
                return Err("a version snapshot's parts arrived out of order".into());
            }
            pending.bytes = pending.bytes.checked_add(bytes).ok_or("version snapshot size overflow")?;
            if pending.bytes > crate::wire::PARTS_AT_MOST {
                return Err("the version snapshot exceeds the multipart message budget".into());
            }
            pending.next = pending.next.checked_add(1).ok_or("version snapshot index overflow")?;
            if part.stable.is_some() {
                pending.network.stable = part.stable;
            }
            if part.beta.is_some() {
                pending.network.beta = part.beta;
            }
            pending.network.machines.extend(part.machines);
            pending.network.due.extend(part.due);
            pending.network.repairs.extend(part.repairs);
        }
        Ok(if last { self.pending.take().map(|pending| pending.network) } else { None })
    }
}
