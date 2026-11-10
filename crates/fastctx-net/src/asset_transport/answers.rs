//! Pure reads can reduce a page before sending; accepted operations never do.
use super::server::Endpoint;
use fastctx_sync::{identity::OperationId, store::Error};
use fastctx_types::sync::transfer::Message;
use std::io::Write;

pub async fn answer(endpoint: &dyn Endpoint, request: &Message, bytes: u32) -> Result<Message, Error> {
    let mut request = request.clone();
    if let Message::GetScopes { request } = &mut request {
        request.max_bytes = request.max_bytes.min((bytes / 2).max(1));
    }
    loop {
        endpoint.current()?;
        let response = endpoint.answer(request.clone()).await?;
        if fits(&response, bytes as usize)? {
            return Ok(response);
        }
        match &mut request {
            Message::GetScopes { request } if request.max_items > 1 => request.max_items /= 2,
            Message::Source { batch, .. } => {
                #[derive(serde::Deserialize)]
                struct Operation {
                    operation: OperationId,
                }
                let operation = serde_json::from_slice::<Operation>(&batch.proof.data.0)
                    .map_err(|e| Error::Invalid(e.to_string()))?
                    .operation;
                return Err(Error::OutcomeUnknown { operation });
            }
            Message::Submit { request } => return Err(Error::OutcomeUnknown { operation: request.operation.clone() }),
            Message::PublishSourceManifest { upload, .. } => {
                return Err(Error::OutcomeUnknown { operation: upload.operation.clone() });
            }
            _ => return Err(Error::Invalid("one metadata result exceeds its negotiated page bound".into())),
        }
    }
}
fn fits(message: &Message, limit: usize) -> Result<bool, Error> {
    let mut counter = Counter { bytes: 0, limit, full: false };
    match serde_json::to_writer(&mut counter, message) {
        Ok(()) => Ok(true),
        Err(_) if counter.full => Ok(false),
        Err(_) => Err(Error::Invalid("metadata result could not be encoded".into())),
    }
}
struct Counter {
    bytes: usize,
    limit: usize,
    full: bool,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self.bytes.checked_add(bytes.len()).filter(|n| *n <= self.limit) {
            Some(n) => {
                self.bytes = n;
                Ok(bytes.len())
            }
            None => {
                self.full = true;
                Err(std::io::Error::other("metadata page bound"))
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn failed_delivery(request: &Message, error: Error) -> Error {
    let operation = match request {
        Message::Submit { request } => Some(request.operation.clone()),
        Message::PublishSourceManifest { upload, .. } => Some(upload.operation.clone()),
        Message::Source { batch, .. } => {
            #[derive(serde::Deserialize)]
            struct Operation {
                operation: OperationId,
            }
            serde_json::from_slice::<Operation>(&batch.proof.data.0).ok().map(|value| value.operation)
        }
        _ => None,
    };
    operation.map(|operation| Error::OutcomeUnknown { operation }).unwrap_or(error)
}
