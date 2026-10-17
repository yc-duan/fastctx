//! Short filesystem transactions on a conversation's records: one at a time
//! on this machine, so a record read and then written is not changed by
//! another in between, and the order of session events is given out once.

use std::sync::LazyLock;

use fastctx_platform::doing::SyncMutexGuard;

static TRANSITION: LazyLock<crate::ranked::Records> = LazyLock::new(Default::default);

pub fn lock() -> SyncMutexGuard<'static, ()> {
    TRANSITION.lock()
}
