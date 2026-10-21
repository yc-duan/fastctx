//! Device reads keep one authenticated data block queued, independent of
//! the control connection's heartbeat and other calls.

use fastctx_net::file_read::Part;
use fastctx_types::access::Written;
use fastctx_types::file_read::{FileRange, ReadFileRequest};
use fastctx_types::ipc::Failure;
use tokio::sync::mpsc;

use super::{Device, failed};
use crate::{CallError, Error};

pub(crate) struct Reading {
    parts: mpsc::Receiver<Result<Part, Failure>>,
    task: tokio::task::JoinHandle<()>,
}

impl Device {
    pub(crate) async fn read_attachment(
        &self,
        attachment: fastctx_types::attachments::AttachmentSource,
        range: FileRange,
    ) -> Result<Reading, Error> {
        let machine = attachment.owner().machine.clone();
        let session = self.reaching(&machine.clone().into())?;
        session.account_for(&machine, fastctx_types::access::Purpose::Agents)?;
        if session.link.view().member(&machine).is_some_and(|member| !member.online) {
            return Err(CallError::Failed(Failure::not_run(format!(
                "the original is on {machine}; {machine} is currently offline"
            )))
            .into());
        }
        let accounts = {
            let view = session.link.view();
            let me = session.link.membership().name.clone();
            view.me()
                .into_iter()
                .flat_map(|member| &member.logins)
                .filter(|account| view.admits(&me, account, &machine, fastctx_types::access::Purpose::Agents).is_ok())
                .cloned()
                .collect::<Vec<_>>()
        };
        let (sender, parts) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            let reading = async {
                let mut refused = None;
                for account in accounts {
                    match fastctx_net::file_read::read_attachment(
                        &session.link,
                        machine.clone(),
                        account,
                        attachment.clone(),
                        range.clone(),
                        &sender,
                    )
                    .await
                    {
                        Err(error) if error.not_run => refused = Some(error),
                        result => return result,
                    }
                }
                Err(refused.unwrap_or_else(|| {
                    Failure::not_run("this device's accounts changed before reading the attachment")
                }))
            };
            tokio::select! { () = sender.closed() => {}, result = reading => { let _ = sender.send(result.map(Part::Finished)).await; } }
        });
        Ok(Reading { parts, task })
    }
    pub(crate) async fn read_file(&self, machine: Option<Written>, request: ReadFileRequest) -> Result<Reading, Error> {
        let machine =
            machine.ok_or_else(|| failed("choose the machine that has the file; a device has no local node"))?;
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let account = session.account_for(&machine, fastctx_types::access::Purpose::Hands)?;
        // One queued block is the same backpressure boundary as local IPC.
        let (sender, parts) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            tokio::select! {
                () = sender.closed() => {},
                result = fastctx_net::file_read::read(&session.link, machine, account, request, &sender) => {
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
