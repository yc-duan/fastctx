//! Request budgets checked before a recognized call takes a processing place.

use std::io::{self, Write};

use fastctx_types::ipc::Failure;
use fastctx_types::remote::RemoteCall;

use crate::envelope::Envelope;

// No kind's budget is more than a machine assembles of one call.
const _: () = assert!(fastctx_types::contract::REQUEST_BYTES <= crate::data::MAX_BODY);

/// Counts the original JSON value, including fields this release does not use.
/// The writer stops at the budget rather than allocating another request copy.
pub(super) fn small(call: &RemoteCall, message: &Envelope) -> Result<(), Failure> {
    let limit = call.contract().max_request_bytes;
    let mut room = Room(limit);
    let request = message.body.get("call").expect("a parsed call has its request");
    serde_json::to_writer(&mut room, request).map_err(|_| too_large(call, limit))
}

/// A large request's encoded size is retained by the transport while it is read.
pub(super) fn large(call: &RemoteCall, bytes: usize) -> Result<(), Failure> {
    let limit = call.contract().max_request_bytes;
    if bytes > limit { Err(too_large(call, limit)) } else { Ok(()) }
}

fn too_large(call: &RemoteCall, limit: usize) -> Failure {
    Failure::not_run(format!("{} request exceeds its {limit}-byte input budget", call.kind()))
}

struct Room(usize);

impl Write for Room {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::Error::other("the request exceeds its input budget"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
