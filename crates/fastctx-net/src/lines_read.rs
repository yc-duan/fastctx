//! Requested logical rows travel in signed pieces on a data connection.

use crate::link::{Feed, Link};
use crate::sign::{Domain, Signed};
use fastctx_types::access::Purpose;
use fastctx_types::ipc::Failure;
use fastctx_types::lines::{Part, Reply, Result as Answer};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, MachineName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub enum Received {
    Text(Part),
    Finished(Reply),
}
pub type Sender = mpsc::Sender<Result<Received, Failure>>;

#[derive(Serialize, Deserialize)]
pub struct Piece {
    pub read: u64,
    pub n: u64,
    pub source: String,
    pub part: Content,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Content {
    Text { part: Part },
    Finished { start: u64, count: u64 },
}

pub async fn read(
    link: &Arc<Link>,
    machine: MachineName,
    account: AccountName,
    request: fastctx_types::lines::ReadRequest,
    sender: &Sender,
    only_notes: bool,
) -> Result<Reply, Failure> {
    let source = request.source.clone();
    let read = u64::from_le_bytes(crate::sign::random::<8>().map_err(|error| Failure::new(error.to_string()))?);
    let (complete, completed) = oneshot::channel();
    let (receiving, from, sender, held_source) = (link.clone(), machine.clone(), sender.clone(), source.clone());
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let mut n = 0;
            loop {
                let signed: Signed = stream.hear().await?;
                let piece: Piece = receiving.unseal(&from, Domain::Event, &signed).await?;
                if piece.read != read || piece.n != n || piece.source != held_source {
                    return Err("out-of-order piece for a different line read".into());
                }
                n += 1;
                match piece.part {
                    Content::Text { part } => {
                        sender.send(Ok(Received::Text(part))).await.map_err(|error| error.to_string())?
                    }
                    Content::Finished { start, count } => {
                        let _ = complete.send((start, count));
                        return stream.done().await;
                    }
                }
            }
        })
    });
    let call =
        if only_notes { RemoteCall::ReadLineNotes { request, read } } else { RemoteCall::ReadLines { request, read } };
    let reply = link
        .call_with_stream(&machine, &account, Purpose::Hands, call, feed)
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    let RemoteReply::Lines(reply) = reply else {
        return Err(Failure::new("expected a line-read completion"));
    };
    let (returned_start, returned_count) =
        completed.await.map_err(|_| Failure::new("the line read ended without its signed completion"))?;
    if reply.machine != machine
        || reply.result != (Answer::ReadComplete { start: returned_start, count: returned_count })
    {
        return Err(Failure::new("the line read's completion disagrees with its control reply"));
    }
    Ok(reply)
}
