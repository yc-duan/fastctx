//! A live data connection owns a remote disposable line index.

use crate::link::{Feed, Link};
use crate::sign::{Domain, Signed};
use fastctx_types::access::Purpose;
use fastctx_types::ipc::Failure;
use fastctx_types::lines::{Decoding, Reply, Request};
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName};
use std::sync::Arc;

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
    path: String,
    decoding: Decoding,
) -> Result<(Reply, Lease), Failure> {
    let (opened, opening) = tokio::sync::oneshot::channel();
    let (reading, from) = (link.clone(), machine.clone());
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let signed: Signed = stream.hear().await?;
            let reply: Reply = reading.unseal(&from, Domain::Event, &signed).await?;
            if reply.machine != from {
                return Err("line source named a different machine".into());
            }
            let _ = opened.send(reply);
            stream.closed().await;
            Ok(())
        })
    });
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let call = link.call_with_stream(
            &machine,
            &account,
            Purpose::Hands,
            RemoteCall::Lines { request: Request::Open { path, decoding } },
            feed,
        );
        tokio::pin!(call);
        let result = tokio::select! {
            opened = opening => opened.map_err(|error| Failure::new(error.to_string())),
            result = &mut call => Err(match result { Ok(Err(error)) => error, Err(error) => Failure::new(error.to_string()), Ok(Ok(_)) => Failure::new("line source ended before opening") }),
        };
        let failed = result.is_err();
        if sender.send(result).is_err() || failed {
            return;
        }
        let _ = call.await;
    });
    let lease = Lease(task);
    Ok((receiver.await.map_err(|error| Failure::new(error.to_string()))??, lease))
}
