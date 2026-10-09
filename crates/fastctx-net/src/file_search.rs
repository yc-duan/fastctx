//! A file search is owned by a live data connection, including while it opens.

use crate::link::{Feed, Link};
use crate::sign::{Domain, Signed};
use fastctx_types::access::Purpose;
use fastctx_types::file_search::{FileSearchReply, FileSearchRequest, FileSearchScope};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName};
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct Lease(tokio::task::JoinHandle<()>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Lease {
    pub async fn closed(&mut self) {
        let _ = (&mut self.0).await;
    }
}

pub async fn open(
    link: Arc<Link>,
    machine: MachineName,
    account: AccountName,
    scope: FileSearchScope,
) -> Result<(FileSearchReply, Lease), Failure> {
    let (sender, receiver) = oneshot::channel();
    let (reading, from) = (link.clone(), machine.clone());
    let (opened, opening) = oneshot::channel();
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let signed: Signed = stream.hear().await?;
            let reply: FileSearchReply = reading.unseal(&from, Domain::Event, &signed).await?;
            if reply.machine != from {
                return Err("the search named a different machine".into());
            }
            let _ = opened.send(reply);
            stream.closed().await;
            Ok(())
        })
    });
    let task = tokio::spawn(async move {
        let open = RemoteCall::FindFiles { request: FileSearchRequest::Open { scope } };
        let call = link.call_with_stream(&machine, &account, Purpose::Hands, open, feed);
        tokio::pin!(call);
        let result = tokio::select! {
            reply = opening => reply.map_err(|error| Failure::new(format!("opening the file search: {error}"))),
            result = &mut call => Err(match result {
                Ok(Err(error)) => error,
                Err(error) => Failure::new(error.to_string()),
                Ok(Ok(_)) => Failure::new("the file search ended before opening"),
            }),
        };
        let failed = result.is_err();
        if sender.send(result).is_err() || failed {
            return;
        }
        let _ = call.await;
    });
    let lease = Lease(task);
    let reply = receiver.await.map_err(|error| Failure::new(error.to_string()))??;
    Ok((reply, lease))
}
