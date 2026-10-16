//! A machine's unsent changes, indexed by id. Deletions stay until sent:
//! the reader may already know an id that came and went while it waited.

use std::collections::BTreeMap;

use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use fastctx_types::town::{Afloat, AfootChange, ResidentNow, RunningJob};
use serde::Serialize;

/// The charge for all queued afoot notices, including the one being drained.
/// It includes JSON payloads, stored keys and values, and a header per notice;
/// tree allocation overhead is proportional to this, not included byte for byte.
pub(super) const LIMIT: usize = 32 * 1024 * 1024;
/// Room for a notice's header, machine name and collection handles.
const HEADER: usize = 1024;
/// Room for the change's braces, list names and `anew`, as in the wire splitter.
const BARE: usize = 128;

pub(super) struct Pending {
    machine: MachineName,
    busy: Option<u32>,
    anew: bool,
    residents: Changes<ResidentNow>,
    jobs: Changes<RunningJob>,
}

impl Pending {
    /// Reserves a notice's header in the session's shared charge.
    pub(super) fn new(machine: MachineName, bytes: &mut usize) -> Result<Self, &'static str> {
        if *bytes > LIMIT - HEADER {
            return Err("afoot notices exceed their pending byte budget");
        }
        *bytes += HEADER;
        Ok(Self { machine, busy: None, anew: false, residents: Changes::default(), jobs: Changes::default() })
    }

    /// Incorporates a change in time proportional to its size and logarithmic
    /// in the number of retained ids. Failure ends the session; a partial
    /// change must never be sent after it.
    pub(super) fn merge(
        &mut self,
        change: AfootChange<ResidentNow, RunningJob>,
        busy: Option<u32>,
        bytes: &mut usize,
    ) -> Result<(), &'static str> {
        if change.anew {
            self.residents.clear(bytes);
            self.jobs.clear(bytes);
            self.anew = true;
        }
        for id in change.residents_gone {
            self.residents.put(id, None, self.anew, bytes)?;
        }
        for resident in change.residents {
            self.residents.put(resident.id(), Some(resident), self.anew, bytes)?;
        }
        for id in change.jobs_gone {
            self.jobs.put(id, None, self.anew, bytes)?;
        }
        for job in change.jobs {
            self.jobs.put(job.id(), Some(job), self.anew, bytes)?;
        }
        self.busy = busy;
        Ok(())
    }

    /// Takes one wire frame and releases only its charge. Once draining starts,
    /// later changes must wait separately until this notice is fully sent.
    pub(super) fn take(&mut self, bytes: &mut usize) -> (Down, bool) {
        let mut room = fastctx_net::afoot::FRAME_ROOM - BARE;
        let (residents, residents_gone) = self.residents.take(&mut room, bytes);
        let (jobs, jobs_gone) = self.jobs.take(&mut room, bytes);
        let change = AfootChange { anew: std::mem::take(&mut self.anew), residents, residents_gone, jobs, jobs_gone };
        let done = self.residents.entries.is_empty() && self.jobs.entries.is_empty();
        if done {
            *bytes -= HEADER;
        }
        (Down::Afoot { machine: self.machine.clone(), busy: self.busy, change: Box::new(change) }, done)
    }
}

struct Entry<T> {
    value: Option<Box<T>>,
    /// JSON bytes, with one comma, for forming a wire frame without encoding again.
    room: usize,
    /// Its charge, including the stored key and value.
    bytes: usize,
}

struct Changes<T: Afloat> {
    entries: BTreeMap<T::Id, Entry<T>>,
    bytes: usize,
}

impl<T: Afloat> Default for Changes<T> {
    fn default() -> Self {
        Self { entries: BTreeMap::new(), bytes: 0 }
    }
}

impl<T> Changes<T>
where
    T: Afloat + Serialize,
    T::Id: Serialize,
{
    fn clear(&mut self, bytes: &mut usize) {
        *bytes -= self.bytes;
        self.bytes = 0;
        self.entries.clear();
    }

    fn put(&mut self, id: T::Id, value: Option<T>, anew: bool, bytes: &mut usize) -> Result<(), &'static str> {
        // An unsent reset already removes everything the reader used to know.
        if anew && value.is_none() {
            if let Some(old) = self.entries.remove(&id) {
                self.bytes -= old.bytes;
                *bytes -= old.bytes;
            }
            return Ok(());
        }
        let id_bytes = size(&id);
        let room = value.as_ref().map_or(id_bytes, size) + 1;
        if room > fastctx_net::afoot::FRAME_ROOM - BARE {
            return Err("an afoot item is larger than a control frame");
        }
        let charge =
            size_of::<(T::Id, Entry<T>)>() + room + if value.is_some() { size_of::<T>() + id_bytes } else { 0 };
        let old = self.entries.get(&id).map_or(0, |old| old.bytes);
        if charge > LIMIT - (*bytes - old) {
            return Err("afoot notices exceed their pending byte budget");
        }
        *bytes = *bytes - old + charge;
        self.bytes = self.bytes - old + charge;
        self.entries.insert(id, Entry { value: value.map(Box::new), room, bytes: charge });
        Ok(())
    }

    fn take(&mut self, room: &mut usize, bytes: &mut usize) -> (Vec<T>, Vec<T::Id>) {
        let mut values = Vec::new();
        let mut gone = Vec::new();
        while let Some((_, entry)) = self.entries.first_key_value() {
            if entry.room > *room {
                break;
            }
            let (id, entry) = self.entries.pop_first().expect("looked at above");
            *room -= entry.room;
            *bytes -= entry.bytes;
            self.bytes -= entry.bytes;
            match entry.value {
                Some(value) => values.push(*value),
                None => gone.push(id),
            }
        }
        (values, gone)
    }
}

fn size(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).expect("afoot values serialize").len()
}
