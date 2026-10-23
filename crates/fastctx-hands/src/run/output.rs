//! Draining a process stream through EOF into its durable log.

use std::fs::File;
use std::io::{self, Write as _};

use fastctx_platform::output_pipe::{Read, Reader};
use fastctx_types::run::OutputLost;

/// After a write error, continue draining so a full disk cannot deadlock
/// the process. Record the exact successfully written prefix and the error.
pub(super) fn copy(
    mut output: Reader,
    mut log: File,
    limit: Option<u64>,
    drained: impl FnOnce(u64, Option<OutputLost>),
) -> io::Result<Option<OutputLost>> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut kept = 0u64;
    let mut lost = None;
    let mut drained = Some(drained);
    let mut pending = None;
    loop {
        if pending == Some(0)
            && let Some(drained) = drained.take()
        {
            drained(kept, lost.clone());
        }
        let capacity = pending.filter(|n| *n > 0).unwrap_or(buffer.len()).min(buffer.len());
        let read = match output.next(&mut buffer[..capacity]) {
            Ok(Read::Checkpoint(queued)) => {
                pending = Some(queued);
                continue;
            }
            Ok(Read::Bytes(0)) => {
                if let Some(drained) = drained.take() {
                    drained(kept, lost.clone());
                }
                return Ok(lost);
            }
            Ok(Read::Bytes(read)) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if let Some(pending) = pending.as_mut() {
            *pending = pending.saturating_sub(read);
        }
        if limit.is_some_and(|limit|kept.saturating_add(read as u64)>limit) {
            return Err(io::Error::other("sentinel observation output exceeded 65536 bytes"));
        }
        let mut bytes = &buffer[..read];
        while lost.is_none() && !bytes.is_empty() {
            match log.write(bytes) {
                Ok(0) => {
                    lost = Some(OutputLost {
                        kept_bytes: kept,
                        error: io::Error::from(io::ErrorKind::WriteZero).to_string(),
                    })
                }
                Ok(written) => {
                    kept += written as u64;
                    bytes = &bytes[written..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => lost = Some(OutputLost { kept_bytes: kept, error: error.to_string() }),
            }
        }
    }
}
