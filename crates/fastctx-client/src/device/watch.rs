//! The existing signed watch stream, subscribed to directly by the device.

use fastctx_types::access::Written;
use std::sync::Arc;

use fastctx_net::link::{Feed, Link};
use fastctx_net::sign::Domain;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::session::Watched;
use fastctx_types::{MachineName, ResidentId};
use serde::Deserialize;
use tokio::sync::mpsc;

use super::{Device, Session};
use crate::{CallError, Error};

/// The shape signed by `fastctx-node::talk::serve_watch`. It is bound to
/// one randomly named subscription, with consecutive sequence numbers.
#[derive(Deserialize)]
struct Piece {
    watch: u64,
    n: u64,
    watched: Watched,
}

pub(crate) struct Watching {
    pieces: mpsc::UnboundedReceiver<Result<Watched, CallError>>,
    task: tokio::task::JoinHandle<()>,
}

impl Device {
    pub(crate) async fn watch(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        composer: Option<String>,
        held: Option<fastctx_types::session::held::Held>,
        now: bool,
        goal: Option<fastctx_types::access::HubAccount>,
    ) -> Result<Watching, Error> {
        let machine = self.find(&resident, machine).await?;
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let scoped = goal.is_some();
        let account = match goal {
            Some(goal) => {
                if goal.hub != session.link.membership().id { return Err(Error::Unexpected("the goal account belongs to another hub".into())); }
                session.link.view().admits(&session.link.membership().name, &goal.account, &machine, fastctx_types::access::Purpose::Agents)
                    .map_err(|error| Error::Unexpected(error.to_string()))?;
                goal.account
            }
            None => session.account_for(&machine, fastctx_types::access::Purpose::Agents)?,
        };
        Watching::start(session, machine, account, resident, composer, held, now, scoped)
    }
}

impl Watching {
    #[allow(clippy::too_many_arguments)]
    fn start(
        session: Arc<Session>,
        machine: MachineName,
        account: fastctx_types::AccountName,
        resident: ResidentId,
        composer: Option<String>,
        held: Option<fastctx_types::session::held::Held>,
        now: bool,
        goal: bool,
    ) -> Result<Self, Error> {
        let watch = u64::from_le_bytes(fastctx_net::sign::random::<8>()?);
        let (sender, pieces) = mpsc::unbounded_channel();
        let (feeding, link, from) = (sender.clone(), session.link.clone(), machine.clone());
        let feed = feed(link, from, watch, feeding);
        let task = tokio::spawn(async move {
            let ended = tokio::select! {
                () = sender.closed() => return,
                result = session.link.call_with_stream(&machine, &account, fastctx_types::access::Purpose::Agents, RemoteCall::Watch { resident, watch, composer, held, now, goal }, feed) => result,
            };
            let failure = match ended {
                Ok(Ok(RemoteReply::Watched)) => return,
                Ok(Ok(_)) => Failure::new(format!("{machine} answered something other than a watch")),
                Ok(Err(failure)) => failure,
                Err(failure) => Failure::new(failure.to_string()),
            };
            let _ = sender.send(Err(CallError::Failed(failure)));
        });
        Ok(Self { pieces, task })
    }

    pub async fn next(&mut self) -> Result<Option<Watched>, CallError> {
        self.pieces.recv().await.transpose()
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn feed(
    link: Arc<Link>,
    from: MachineName,
    watch: u64,
    sender: mpsc::UnboundedSender<Result<Watched, CallError>>,
) -> Feed {
    Box::new(move |mut stream| {
        Box::pin(async move {
            let mut n = 0;
            loop {
                let signed = tokio::select! {
                    () = sender.closed() => return Ok(()),
                    signed = fastctx_net::watch_stream::receive(&mut stream) => signed?,
                };
                // Closed by the watched machine: its answer to the watch says why.
                let Some(signed) = signed else { return Ok(()) };
                let piece: Piece = link.unseal_packed(&from, Domain::Event, &signed).await?;
                if piece.watch != watch || piece.n != n {
                    return Err(format!(
                        "{from} sent a piece out of place (watch {:x}, number {}; expected {watch:x}, {n})",
                        piece.watch, piece.n
                    ));
                }
                n += 1;
                if sender.send(Ok(piece.watched)).is_err() {
                    return Ok(());
                }
            }
        })
    })
}
