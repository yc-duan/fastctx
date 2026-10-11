//! A disposable data connection carries cancellation of user-only reads.

use crate::link::{CallFailure, Feed, Link};
use fastctx_types::remote::{RemoteCall, RemoteResult};
use fastctx_types::{AccountName, MachineName};
use std::sync::Arc;

pub async fn call(
    link: &Arc<Link>,
    machine: &MachineName,
    account: &AccountName,
    request: RemoteCall,
) -> Result<RemoteResult, CallFailure> {
    Box::pin(link.call_with_stream(machine, account, request.purpose(), request, feed())).await
}

pub fn feed() -> Feed {
    let feed: Feed = Box::new(|mut stream| {
        Box::pin(async move {
            stream.closed().await;
            Ok(())
        })
    });
    feed
}
