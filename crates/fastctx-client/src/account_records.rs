//! Account records through a node or directly from a phone to its hub.

use crate::{CallError, Client, Error, World};
use fastctx_types::{
    AccountName, Call, HubName, Reply,
    account_records::{Request, Response, Watched},
    ipc::{ClientFrame, Failure, NodeFrame},
};

impl World {
    pub async fn account_records(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        request: Request,
    ) -> Result<Response, Error> {
        match self.ask(Call::AccountRecords { hub, account, request }).await? {
            Reply::AccountRecords { response } => Ok(response),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn watch_account_records(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        kind: String,
    ) -> Result<RecordsWatch, Error> {
        if let Some(device) = self.device() {
            return Ok(RecordsWatch::Device(device.record_watch(hub.as_ref(), account, kind)?));
        }
        let mut client = self.connect().await?;
        if !client.account_records {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::account_records::words::NODE_OLD)).into());
        }
        client.next += 1;
        let id = client.next;
        let call = Call::WatchAccountRecords { hub, account, kind };
        client.connection.send(&crate::to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        Ok(RecordsWatch::Local { client, id })
    }
}

pub enum RecordsWatch {
    Local { client: Client, id: u64 },
    Device(fastctx_net::link::account_records::RecordWatch),
}

impl RecordsWatch {
    /// A fresh listing on open, then metadata invalidations. Reopen after an
    /// error to recover changes made while the connection was unavailable.
    pub async fn next(&mut self) -> Result<Watched, Error> {
        match self {
            Self::Device(watch) => watch.next().await.map_err(|why| CallError::Failed(Failure::new(why)).into()),
            Self::Local { client, id } => match client.frame().await? {
                NodeFrame::AccountRecords { id: received, event } if received == *id => Ok(event),
                NodeFrame::Reply { id: received, reply: Err(failure) } if received == *id => {
                    Err(CallError::Failed(failure).into())
                }
                other => Err(Error::Unexpected(format!("{other:?}"))),
            },
        }
    }
}
