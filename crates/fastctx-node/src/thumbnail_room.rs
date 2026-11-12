//! Room for decoding pictures. The OS, the resident host, the node and the
//! decoders each get a quarter of the memory: decoders together take at
//! most that quarter, counted in the allowance each asks for, and at most a
//! few at once. Only the decoding waits for room; finding the picture, its
//! dimensions and its fingerprint do not, so a large picture holds up
//! neither those nor small pictures that fit beside it.

use std::sync::LazyLock;

use fastctx_types::ipc::Failure;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MIB: u64 = 1 << 20;

struct Room {
    /// The decoders' quarter of the memory, a permit for each MiB.
    mebibytes: std::sync::Arc<Semaphore>,
    /// How many may decode at once.
    at_once: std::sync::Arc<Semaphore>,
    quarter: u64,
}

static ROOM: LazyLock<Room> = LazyLock::new(|| {
    let quarter = fastctx_platform::memory::physical().unwrap_or(1 << 30) / 4;
    let mebibytes = (quarter / MIB).clamp(1, u64::from(u32::MAX) / 2) as usize;
    let at_once = std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 4);
    Room {
        mebibytes: std::sync::Arc::new(Semaphore::new(mebibytes)),
        at_once: std::sync::Arc::new(Semaphore::new(at_once)),
        quarter,
    }
});

/// Room one decoder holds while it runs.
pub(crate) struct Taken {
    _mebibytes: OwnedSemaphorePermit,
    _at_once: OwnedSemaphorePermit,
}

/// Waits for room to decode with an allowance of `budget` bytes; refused at
/// once when the allowance is more than the decoders' whole quarter.
pub(crate) async fn take(budget: u64) -> Result<Taken, Failure> {
    let room = &*ROOM;
    if budget > room.quarter {
        return Err(Failure::new(format!(
            "image decoding requires a {budget}-byte allowance; this machine can give {}",
            room.quarter
        )));
    }
    let wanted = budget.div_ceil(MIB).clamp(1, (room.quarter / MIB).clamp(1, u64::from(u32::MAX) / 2));
    let wanted = u32::try_from(wanted).unwrap_or(u32::MAX);
    let at_once = room.at_once.clone().acquire_owned().await.map_err(|error| Failure::new(error.to_string()))?;
    let mebibytes =
        room.mebibytes.clone().acquire_many_owned(wanted).await.map_err(|error| Failure::new(error.to_string()))?;
    Ok(Taken { _mebibytes: mebibytes, _at_once: at_once })
}
