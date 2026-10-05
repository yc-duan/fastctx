//! The user's original-byte reads, with bounded backpressure all the way
//! from the interface through the data connection to the opened file.

mod ipc;
mod local;
mod remote;

use std::sync::Arc;

use fastctx_types::MachineName;
use fastctx_types::file_read::*;
use fastctx_types::ipc::Failure;
use tokio::sync::mpsc;

use crate::Node;

use fastctx_net::file_read::Part;

type Sender = mpsc::Sender<Result<Part, Failure>>;
type Receiver = mpsc::Receiver<Result<Part, Failure>>;

fn start(node: Arc<Node>, machine: Option<MachineName>, request: ReadFileRequest) -> Receiver {
    // At most one data-message payload waits between the file/network and
    // IPC. Slow interfaces therefore stop the producer instead of buffering.
    let (sender, receiver) = mpsc::channel(1);
    tokio::spawn(async move {
        tokio::select! {
            () = sender.closed() => {},
            result = async {
                match machine.filter(|machine| *machine != node.machine()) {
                    None => {
                        let path = request.path.clone();
                        Ok(local::read(node.machine(), request, &sender).await.map_err(|mut error| {
                            error.message = format!("{}:{path}: {}", node.machine(), error.message);
                            error
                        }))
                    }
                    Some(machine) => match node.link() {
                        Some(link) => fastctx_net::file_read::read(&link, machine, request, &sender).await,
                        None => Err(Failure::new(format!("{} is in no World with {machine}", node.machine()))),
                    },
                }
            } => { let _ = sender.send(result.map(Part::Finished)).await; }
        }
    });
    receiver
}

pub(super) use ipc::serve as serve_local;
pub(super) use remote::serve;
