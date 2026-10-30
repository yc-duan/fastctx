//! A version notice retains its snapshot and materializes one bounded frame
//! at a time. Replacing it restarts at zero, superseding any incomplete view.
use fastctx_net::wire::{self, Down};
use fastctx_types::{MachineName, update::Network};
use serde::Serialize;
use std::{collections::BTreeMap, io, ops::Bound, sync::Arc};

/// A shared visible network and the full arrangements needed by its executor.
#[derive(Clone)]
pub(crate) struct Snapshot {
    network: Arc<Network>,
    name: Option<MachineName>,
    update: Option<Arc<crate::store::DueFor>>,
    repair: Option<Arc<crate::store::RepairFor>>,
}
impl Snapshot {
    pub(crate) fn new(
        network: Arc<Network>,
        name: Option<MachineName>,
        update: Option<Arc<crate::store::DueFor>>,
        repair: Option<Arc<crate::store::RepairFor>>,
    ) -> Self {
        Self { network, name, update, repair }
    }
    pub(crate) fn network(&self) -> &Arc<Network> {
        &self.network
    }
    pub(crate) fn materialize(&self) -> Network {
        let mut network = (*self.network).clone();
        if let Some(name) = &self.name {
            if let Some(update) = &self.update {
                network.due.insert(name.clone(), update.due.clone());
            }
            if let Some(repair) = &self.repair {
                network.repairs.insert(name.clone(), repair.due.clone());
            }
        }
        network
    }
}
impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && (Arc::ptr_eq(&self.network, &other.network) || self.network == other.network)
            && match (&self.update, &other.update) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a.due == b.due,
                (None, None) => true,
                _ => false,
            }
            && match (&self.repair, &other.repair) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a.due == b.due,
                (None, None) => true,
                _ => false,
            }
    }
}

pub(super) struct Pending {
    snapshot: Snapshot,
    parts: bool,
    index: u32,
    phase: u8,
    after: Option<MachineName>,
}

impl Pending {
    pub(super) fn new(snapshot: Snapshot, parts: bool) -> Self {
        Self { snapshot, parts, index: 0, phase: 0, after: None }
    }

    pub(super) fn take(&mut self) -> Result<(Down, bool), String> {
        if self.index == 0 {
            let base_size = size(&*self.snapshot.network);
            if base_size + 64 <= wire::MAX_CONTROL_FRAME {
                let whole = self.snapshot.materialize();
                if size(&whole) + 64 <= wire::MAX_CONTROL_FRAME {
                    return Ok((Down::Versions { network: Box::new(whole) }, true));
                }
            }
            if !self.parts {
                return Err("the network's version snapshot exceeds one control frame; update this machine's Fastctx to read it in parts".into());
            }
            let details = self.snapshot.update.as_ref().map_or(0, |u| size(&u.due))
                + self.snapshot.repair.as_ref().map_or(0, |r| size(&r.due));
            if base_size + details > wire::PARTS_AT_MOST {
                return Err("the network's version snapshot exceeds the multipart message budget".into());
            }
        }
        let mut part = if self.index == 0 {
            Network {
                policy: self.snapshot.network.policy.clone(),
                hub: self.snapshot.network.hub.clone(),
                updates_view: self.snapshot.network.updates_view,
                repair_supported: self.snapshot.network.repair_supported,
                ..Network::default()
            }
        } else {
            Network::default()
        };
        let mut bytes = size(&part) + 128;
        if bytes > wire::MAX_CONTROL_FRAME {
            return Err("the network's update policy exceeds one control frame".into());
        }
        let mut added = false;
        while self.phase < 7 {
            let more = match self.phase {
                0 => self.snapshot.network.stable.as_ref().map(|value| size(value) + 32),
                1 => self.snapshot.network.beta.as_ref().map(|value| size(value) + 32),
                2 => next(&self.snapshot.network.machines, &self.after).map(|row| size(&row) + 32),
                3 => next(&self.snapshot.network.due, &self.after).map(|row| size(&row) + 32),
                4 => next(&self.snapshot.network.repairs, &self.after).map(|row| size(&row) + 32),
                5 => self.snapshot.update.as_ref().map(|row| size(&(&self.snapshot.name, &row.due)) + 32),
                6 => self.snapshot.repair.as_ref().map(|row| size(&(&self.snapshot.name, &row.due)) + 32),
                _ => unreachable!(),
            };
            let Some(more) = more else {
                self.phase += 1;
                self.after = None;
                continue;
            };
            if bytes + more > wire::MAX_CONTROL_FRAME {
                if !added && self.index != 0 {
                    return Err("one network version record exceeds a control frame".into());
                }
                break;
            }
            bytes += more;
            added = true;
            match self.phase {
                0 => {
                    part.stable = self.snapshot.network.stable.clone();
                    self.phase += 1;
                }
                1 => {
                    part.beta = self.snapshot.network.beta.clone();
                    self.phase += 1;
                }
                2 => insert_next(&self.snapshot.network.machines, &mut self.after, &mut part.machines),
                3 => insert_next(&self.snapshot.network.due, &mut self.after, &mut part.due),
                4 => insert_next(&self.snapshot.network.repairs, &mut self.after, &mut part.repairs),
                5 => {
                    part.due.insert(
                        self.snapshot.name.clone().expect("executor name"),
                        self.snapshot.update.as_ref().expect("measured above").due.clone(),
                    );
                    self.phase += 1;
                }
                6 => {
                    part.repairs.insert(
                        self.snapshot.name.clone().expect("executor name"),
                        self.snapshot.repair.as_ref().expect("measured above").due.clone(),
                    );
                    self.phase += 1;
                }
                _ => unreachable!(),
            }
        }
        let last = self.phase == 7;
        let frame = Down::VersionPart { index: self.index, last, network: Box::new(part) };
        self.index = self.index.checked_add(1).expect("version parts are bounded by the snapshot budget");
        Ok((frame, last))
    }
}

fn next<'a, T>(rows: &'a BTreeMap<MachineName, T>, after: &Option<MachineName>) -> Option<(&'a MachineName, &'a T)> {
    match after {
        Some(after) => rows.range((Bound::Excluded(after.clone()), Bound::Unbounded)).next(),
        None => rows.iter().next(),
    }
}
fn insert_next<T: Clone>(
    rows: &BTreeMap<MachineName, T>,
    after: &mut Option<MachineName>,
    part: &mut BTreeMap<MachineName, T>,
) {
    let (name, value) = next(rows, after).expect("measured above");
    part.insert(name.clone(), value.clone());
    *after = Some(name.clone());
}

fn size(value: &impl Serialize) -> usize {
    struct Count(usize);
    impl io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).expect("version records serialize");
    count.0
}
