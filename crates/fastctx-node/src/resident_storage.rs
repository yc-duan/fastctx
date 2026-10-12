//! Short filesystem transactions on a conversation's records: one at a time
//! on this machine, so a record read and then written is not changed by
//! another in between, and the order of session events is given out once.

use std::sync::{Mutex, MutexGuard};

static TRANSITION: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    TRANSITION.lock().expect("never poisoned")
}
