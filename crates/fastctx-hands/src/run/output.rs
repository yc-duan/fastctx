//! Draining a process stream through EOF into its durable log.

use std::fs::File;
use std::io::{self, Read as _, Write as _};

use fastctx_types::run::OutputLost;

/// After a write error, continue draining so a full disk cannot deadlock
/// the process. Record the exact successfully written prefix and the error.
pub(super) fn copy(mut output: io::PipeReader, mut log: File) -> io::Result<Option<OutputLost>> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut kept = 0u64;
    let mut lost = None;
    loop {
        let read = match output.read(&mut buffer) {
            Ok(0) => return Ok(lost),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // Windows reports EOF on an anonymous pipe as BrokenPipe.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(lost),
            Err(error) => return Err(error),
        };
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
