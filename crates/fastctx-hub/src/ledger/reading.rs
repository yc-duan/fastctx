//! One page of a ledger, read without keeping the member's frame reader or
//! any hub lock waiting for disk. Entries are immutable and numbered in
//! order, so the committed end is all a read needs to take under the lock.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use fastctx_net::sign::Signed;
use fastctx_net::wire::{self, Down, Told};
use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use tokio::sync::oneshot;

use crate::outq::Outq;
use crate::{Hub, Out};

const READ_WITHIN: Duration = Duration::from_secs(55);
static READERS: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(32)));

/// The committed end never advances during a page, even if new entries
/// reach disk while it is being read.
pub(super) struct Page {
    pub dir: PathBuf,
    pub town: TownId,
    pub after: u64,
    pub last: u64,
}

/// Answers a ledger read on this session's queue, independently of its
/// next frame and of every project's writer.
pub(crate) fn ask(hub: &Arc<Hub>, name: &MachineName, out: &Outq, id: u64, town: TownId, after: u64) {
    if !hub.store().book.directory.member_keeps_project(name, &town) {
        let answer = Err(format!(
            "{name} has no logged-in account that may keep the bodies of {town}; assisting hosts must use project indices (update older clients)"
        ));
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
        return;
    }
    let place = match READERS.clone().try_acquire_owned() {
        Ok(place) => place,
        Err(_) => {
            let answer = Err("the hub is busy reading ledgers; try again".to_string());
            let _ = out.send(Out::Frame(Down::Told { id, answer }));
            return;
        }
    };
    let deadline = tokio::time::Instant::now() + READ_WITHIN;
    let page = hub.ledgers.lock().expect("never poisoned").page(town.clone(), after);
    let (hub, name, out) = (hub.clone(), name.clone(), out.clone());
    tokio::spawn(async move {
        // Dropping the receiver stops a running read between files. Aborting
        // its task also stops a read still waiting for a blocking thread.
        let (wanted, reader) = oneshot::channel::<()>();
        let mut reading = tokio::task::spawn_blocking(move || {
            // A cancelled caller cannot return a place still occupied by disk I/O.
            let _place = place;
            page.read(&wanted, deadline)
        });
        let answer = tokio::select! {
            biased;
            () = out.closed() => {
                reading.abort();
                return;
            }
            () = tokio::time::sleep_until(deadline) => {
                reading.abort();
                Err(format!("reading {town}'s ledger did not finish within {} seconds", READ_WITHIN.as_secs()))
            }
            read = &mut reading => read
                .map_err(|error| format!("reading {town}'s ledger: {error}"))
                .and_then(|answer| answer),
        };
        drop(reader);
        // Permission may have been withdrawn while the disk was being read.
        let answer = if hub.store().book.directory.member_keeps_project(&name, &town) {
            answer
        } else {
            Err(format!(
                "{name} has no logged-in account that may keep the bodies of {town}; assisting hosts must use project indices (update older clients)"
            ))
        };
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    });
}

impl Page {
    fn read(self, wanted: &oneshot::Sender<()>, deadline: tokio::time::Instant) -> Result<Told, String> {
        // The longest id and `false` leave room for either value of `more`.
        let mut size =
            wire::encode(&Down::Told { id: u64::MAX, answer: Ok(Told::Entries { entries: Vec::new(), more: false }) })
                .len();
        let mut entries = Vec::new();
        for previous in self.after..self.last {
            if wanted.is_closed() || tokio::time::Instant::now() >= deadline {
                return Err("the ledger reader ended".into());
            }
            let seq = previous + 1;
            let path = self.dir.join(format!("{seq:020}.json"));
            let signed = entry(&path).map_err(|error| format!("reading entry {seq} of {}: {error}", self.town))?;
            let cost = wire::encode(&(seq, &signed)).len() + usize::from(!entries.is_empty());
            if size + cost > wire::PARTS_AT_MOST || (!entries.is_empty() && size + cost > wire::MAX_CONTROL_FRAME) {
                if entries.is_empty() {
                    return Err(format!(
                        "entry {seq} of {} needs {} bytes in an answer, more than the multipart limit ({})",
                        self.town,
                        size + cost,
                        wire::PARTS_AT_MOST
                    ));
                }
                return Ok(Told::Entries { entries, more: true });
            }
            size += cost;
            entries.push((seq, signed));
        }
        Ok(Told::Entries { entries, more: false })
    }
}

/// A damaged file must not make a read allocate beyond the multipart limit.
pub(super) fn entry(path: &Path) -> io::Result<Signed> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(wire::PARTS_AT_MOST as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > wire::PARTS_AT_MOST {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the entry file is larger than the multipart limit"));
    }
    serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
