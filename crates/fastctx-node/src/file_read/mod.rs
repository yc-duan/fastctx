//! The user's original-byte reads, with bounded backpressure all the way
//! from the interface through the data connection to the opened file.

mod ipc;
mod local;
mod remote;

use fastctx_types::access::Written;
use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::file_read::*;
use fastctx_types::ipc::Failure;
use tokio::sync::mpsc;

use crate::Node;

use fastctx_net::file_read::Part;

type Sender = mpsc::Sender<Result<Part, Failure>>;
type Receiver = mpsc::Receiver<Result<Part, Failure>>;

fn start(node: Arc<Node>, acting: Acting, machine: Option<Written>, request: ReadFileRequest) -> Receiver {
    // At most one data-message payload waits between the file/network and
    // IPC. Slow interfaces therefore stop the producer instead of buffering.
    let (sender, receiver) = mpsc::channel(1);
    tokio::spawn(async move {
        tokio::select! {
            () = sender.closed() => {},
            result = async {
                match machine.filter(|machine| !node.is_here(machine)) {
                    None => {
                        let path = request.path.clone();
                        Ok(local::read(node.machine(), request, &sender).await.map_err(|mut error| {
                            error.message = format!("{}:{path}: {}", node.machine(), error.message);
                            error
                        }))
                    }
                    Some(machine) => match node.speaking(&acting, &machine, Purpose::Hands).await {
                        Ok((link, account)) => {
                            fastctx_net::file_read::read(&link, machine.name, account, request, &sender).await
                        }
                        Err(failure) => Err(Failure::new(failure.to_string())),
                    },
                }
            } => { let _ = sender.send(result.map(Part::Finished)).await; }
        }
    });
    receiver
}

struct Cancel(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn start_attachment(
    node: Arc<Node>,
    attachment: fastctx_types::attachments::AttachmentSource,
    range: FileRange,
) -> Receiver {
    attachment_for(node, None, attachment, range)
}

/// Reads the attachment `attachment` names from wherever it is kept: for
/// `acting`, or, without one, for whom this machine acts as itself.
pub(crate) fn attachment_for(
    node: Arc<Node>,
    acting: Option<Acting>,
    attachment: fastctx_types::attachments::AttachmentSource,
    range: FileRange,
) -> mpsc::Receiver<Result<Part, Failure>> {
    let (sender, receiver) = mpsc::channel(1);
    tokio::spawn(async move {
        let cancel = Cancel(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        tokio::select! {
            () = sender.closed() => {},
            result = async {
                let (attachment, acting) = match &attachment {
                    fastctx_types::attachments::AttachmentSource::Upload(receipt) =>
                        match crate::attachments::forwarded::original(&node, receipt)? {
                            Some((original, acting)) => (original.into(), acting),
                            None => (attachment, acting.clone().unwrap_or(Acting::Here)),
                        },
                    _ => (attachment, acting.clone().unwrap_or(Acting::Here)),
                };
                let machine = attachment.owner().machine.clone();
                if !node.is_here(&machine.clone().into()) {
                    node.settled(&machine.clone().into()).await;
                    let speakers = node.speakers(&acting, &machine.clone().into(), fastctx_types::remote::RemoteCall::ReadAttachment {
                        attachment: attachment.clone(), range: range.clone(), read: 0,
                    }.purpose()).map_err(|error| Failure::new(error.to_string()))?;
                    let mut refused = None;
                    for (link,account) in speakers {
                        if link.view().member(&machine).is_some_and(|member| !member.online) {
                            refused = Some(Failure::not_run(format!("the original is on {machine}; {machine} is currently offline")));
                            continue;
                        }
                        match fastctx_net::file_read::read_attachment(&link,machine.clone(),account,attachment.clone(),range.clone(),&sender).await {
                            Err(error) if error.not_run => refused = Some(error),
                            result => return result,
                        }
                    }
                    return Err(refused.expect("speakers names at least one account"));
                }
                let file = crate::attachment_files::resolve(&node, attachment, cancel.0.clone()).await?;
                let request = ReadFileRequest { path: crate::world::native(&file.path), range };
                Ok(local::read_attachment(node.machine(), request, Some(file.description.clone()), Some(file.snapshot.clone()), file.version.clone(), &sender).await)
            } => { let _ = sender.send(result.map(Part::Finished)).await; }
        }
    });
    receiver
}

pub(super) use ipc::serve as serve_local;
pub(super) use ipc::serve_attachment as serve_attachment_local;
pub(super) use remote::serve;
pub(super) use remote::serve_attachment;
