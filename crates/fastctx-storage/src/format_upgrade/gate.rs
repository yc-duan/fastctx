//! A process-local completion signal, separate from opening the service listener.
use super::Record;
use crate::Result;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Default)]
pub struct MigrationGate(Arc<State>);
#[derive(Default)]
struct State {
    result: Mutex<Option<Result<Record>>>,
    changed: tokio::sync::Notify,
    blocking: Condvar,
}
impl std::fmt::Debug for MigrationGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationGate").field("finished", &self.finished()).finish()
    }
}
impl MigrationGate {
    /// True after copying/verification has stopped and released its source snapshot.
    /// An error still releases migration and must be reported; it is not a backup.
    pub fn finished(&self) -> bool {
        self.0.result.lock().expect("upgrade completion").is_some()
    }
    /// For dedicated startup-followup threads, never a service or writer thread.
    pub fn wait_blocking(&self) -> Result<Record> {
        let mut result = self.0.result.lock().expect("upgrade completion");
        loop {
            if let Some(result) = &*result {
                return result.clone();
            }
            result = self.0.blocking.wait(result).expect("upgrade completion");
        }
    }
    pub async fn wait(&self) -> Result<Record> {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.0.result.lock().expect("upgrade completion").clone() {
                return result;
            }
            changed.await;
        }
    }
    pub(super) fn finish(&self, result: Result<Record>) {
        *self.0.result.lock().expect("upgrade completion") = Some(result);
        self.0.changed.notify_waiters();
        self.0.blocking.notify_all();
    }
}
