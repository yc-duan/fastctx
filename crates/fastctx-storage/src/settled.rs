//! Blocking ownership of a mutation lasts until its writer has actually finished.

use super::*;

impl Database {
    /// Admits a write with the ordinary bounded budget, then parks until its
    /// actual commit or failure. Unlike `write_now_at`, an admitted operation has
    /// no response deadline. Use only from a blocking worker that must retain
    /// its operation guard through completion, even if its async waiter leaves.
    /// The SQL closure and the shared writer keep their existing contracts.
    pub fn write_now_settled_at<T: Send + 'static>(
        &self,
        priority: Priority,
        bytes: usize,
        write: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        fastctx_platform::roles::blocking("waiting for a durable mutation to finish");
        self.available()?;
        let permit = blocking::wait(self.0.writes.admit(priority, bytes), QUEUE_WAIT)
            .ok_or_else(|| busy("write admission"))??;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        self.submit(
            priority,
            writer::job(
                permit,
                write,
                |_| {},
                move |result| {
                    let _ = send.send(result);
                },
            ),
        )?;
        receive.recv().map_err(|_| unknown())?
    }
}
