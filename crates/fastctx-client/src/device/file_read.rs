//! Device reads keep one authenticated data block queued, independent of
//! the control connection's heartbeat and other calls.

use fastctx_net::file_read::Part;
use fastctx_types::MachineName;
use fastctx_types::file_read::ReadFileRequest;
use fastctx_types::ipc::Failure;
use tokio::sync::mpsc;

use super::{Device, failed};
use crate::{CallError, Error};

pub(crate) struct Reading {
    parts: mpsc::Receiver<Result<Part, Failure>>,
    task: tokio::task::JoinHandle<()>,
}

impl Device {
    pub(crate) async fn read_file(
        &self,
        machine: Option<MachineName>,
        request: ReadFileRequest,
    ) -> Result<Reading, Error> {
        let machine =
            machine.ok_or_else(|| failed("choose the machine that has the file; a device has no local node"))?;
        let session = self.connected()?;
        session.check_identity().await?;
        session.machine(&machine)?;
        // One queued block is the same backpressure boundary as local IPC.
        let (sender, parts) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            tokio::select! {
                () = sender.closed() => {},
                result = fastctx_net::file_read::read(&session.link, machine, request, &sender) => {
                    let _ = sender.send(result.map(Part::Finished)).await;
                }
            }
        });
        Ok(Reading { parts, task })
    }
}

impl Reading {
    pub(crate) async fn next(&mut self) -> Result<Part, Error> {
        self.parts
            .recv()
            .await
            .ok_or_else(|| failed("the file read stopped without a terminal result"))?
            .map_err(|error| CallError::Failed(error).into())
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        self.task.abort();
    }
}
