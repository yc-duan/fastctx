use crate::Error;
use fastctx_types::access::Written;
use fastctx_types::{
    access::Purpose,
    lines::{Decoding, Reply},
};

impl super::Device {
    pub(crate) fn read_lines(
        &self,
        machine: Written,
        source: String,
        start: u64,
        count: u64,
        only_notes: bool,
    ) -> Result<Reading, Error> {
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let account = session.account_for(&machine, Purpose::Hands)?;
        let (sender, parts) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(async move {
            tokio::select! {
                () = sender.closed() => {},
                result = fastctx_net::lines_read::read(&session.link, machine, account, fastctx_types::lines::ReadRequest { source, start, count }, &sender, only_notes) => {
                    let _ = sender.send(result.map(fastctx_net::lines_read::Received::Finished)).await;
                }
            }
        });
        Ok(Reading { parts, task })
    }
    pub(crate) async fn open_lines(
        &self,
        machine: Written,
        path: String,
        decoding: Decoding,
    ) -> Result<(Reply, fastctx_net::lines::Lease), Error> {
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let account = session.account_for(&machine, Purpose::Hands)?;
        fastctx_net::lines::open(session.link.clone(), machine, account, path, decoding)
            .await
            .map_err(|error| crate::CallError::Failed(error).into())
    }
}

pub(crate) struct Reading {
    parts: tokio::sync::mpsc::Receiver<Result<fastctx_net::lines_read::Received, fastctx_types::ipc::Failure>>,
    task: tokio::task::JoinHandle<()>,
}
impl Reading {
    pub(crate) async fn next(&mut self) -> Result<fastctx_net::lines_read::Received, Error> {
        self.parts
            .recv()
            .await
            .ok_or_else(|| super::failed("line reader stopped before completion"))?
            .map_err(|error| crate::CallError::Failed(error).into())
    }
}
impl Drop for Reading {
    fn drop(&mut self) {
        self.task.abort();
    }
}
