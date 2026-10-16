//! What a task answering a call says it does (`fastctx_platform::doing`):
//! the call's kind, read from the start of how it is written, never the rest
//! of it, which can be the user's words.

use std::io::{self, Write};

/// How much of a call's writing is looked at for its kind.
const HEAD: usize = 96;

/// The kind of call a frame carries, as the frame spells it
/// (`"call":"watch"`); the frame is not parsed for it.
pub(crate) fn call_in(frame: &[u8]) -> String {
    const TAG: &[u8] = b"\"call\":\"";
    frame
        .windows(TAG.len())
        .position(|window| window == TAG)
        .and_then(|at| {
            let rest = &frame[at + TAG.len()..];
            let end = rest.iter().take(64).position(|byte| *byte == b'"')?;
            std::str::from_utf8(&rest[..end]).ok().map(|kind| format!("the call {kind}"))
        })
        .unwrap_or_else(|| "a call".to_string())
}

/// The kind of a call tagged `call`, from the start of its writing alone:
/// the tag is written first, and writing stops once that is had.
pub(crate) fn call_of(call: &impl serde::Serialize) -> String {
    let mut head = Head(Vec::with_capacity(HEAD));
    let _ = serde_json::to_writer(&mut head, call);
    call_in(&head.0)
}

/// A writer that takes the first `HEAD` bytes and refuses the rest.
struct Head(Vec<u8>);

impl Write for Head {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let room = HEAD - self.0.len();
        if room == 0 {
            return Err(io::Error::other("enough of the call is read"));
        }
        let taken = bytes.len().min(room);
        self.0.extend_from_slice(&bytes[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
