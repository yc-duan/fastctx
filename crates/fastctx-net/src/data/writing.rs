//! Disk consumers leave runtime threads free to report real receipt work.

use super::Stream;

impl Stream {
    /// Takes exactly `count` bytes through one owned blocking writer at a time.
    /// A transfer or write failure returns the writer so its partial state can
    /// be settled. A panicked writer is reported separately. If the caller
    /// leaves, the blocking operation retains its writer and work lease.
    pub async fn take_writing<W: Send + 'static>(
        &mut self,
        mut count: u64,
        mut writer: W,
        write: fn(&mut W, &[u8]) -> Result<(), String>,
    ) -> Result<(W, Result<(), String>), String> {
        while count != 0 {
            if self.pending.is_empty() {
                match self.due(count).await {
                    Ok(bytes) => self.pending = bytes,
                    Err(reason) => return Ok((writer, Err(reason))),
                }
            }
            let here = count.min(self.pending.len() as u64) as usize;
            let bytes: Vec<u8> = self.pending.drain(..here).collect();
            let writing = self.settling();
            let (back, written) = tokio::task::spawn_blocking(move || {
                let _writing = writing;
                let written = write(&mut writer, &bytes);
                (writer, written)
            })
            .await
            .map_err(|error| format!("the data writer ended: {error}"))?;
            writer = back;
            if written.is_err() {
                return Ok((writer, written));
            }
            fastctx_platform::doing::progress();
            count -= here as u64;
        }
        Ok((writer, Ok(())))
    }
}
