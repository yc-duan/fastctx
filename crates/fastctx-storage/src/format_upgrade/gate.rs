//! A process-local completion signal, separate from opening the service listener.
use super::Record;
use crate::Result;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct MigrationGate(Arc<State>);
#[derive(Default)]
struct State {
    result: Mutex<Option<Result<Record>>>,
    changed: tokio::sync::Notify,
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
    }
}
