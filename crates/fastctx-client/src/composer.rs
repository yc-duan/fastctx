//! Shared input uses the ordinary user call and resident watch transports.

use fastctx_net::watch_queue;
use fastctx_types::access::Written;
use fastctx_types::composer::{Composer, Mutation, Request, Updated};
use fastctx_types::session::{Happened, SessionChange, Watched};
use fastctx_types::{Call, Reply, ResidentId};

use crate::{Error, World};

/// A fresh subscription identity, unrelated to account credentials or membership.
pub fn watch_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(format!("composer-{:032x}", u128::from_be_bytes(bytes)))
}

impl World {
    pub async fn composer(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        request: Request,
    ) -> Result<Updated, Error> {
        match self.ask(Call::Composer { resident, machine, request }).await? {
            Reply::Composer(updated) => Ok(*updated),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn update_composer(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        mutation: Mutation,
    ) -> Result<Updated, Error> {
        self.composer(resident, machine, Request::Mutate { mutation: Box::new(mutation) }).await
    }

    /// The caller chooses a fresh watch ID; queue editing is bound to its lifetime.
    pub async fn watch_composer(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        name: String,
    ) -> Result<(Composing, Composer), Error> {
        let mut watching = self.watch_with_composer(resident, machine, Some(name)).await?;
        let (sender, states) = watch_queue::channel(|state: &Result<Composer, Error>| match state {
            Ok(state) => (watch_queue::size(state), Some("composer".into())),
            Err(error) => (error.to_string().len(), None),
        });
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = sender.closed() => return,
                    piece = watching.next() => {
                        match piece {
                            Ok(Some(Watched::Live { event })) => {
                                if let Happened::Changed { change: SessionChange::ComposerFailed { error }, .. } = &event.happened {
                                    let _ = sender.send(Err(crate::CallError::Failed(fastctx_types::ipc::Failure::new(error.clone())).into()));
                                    return;
                                }
                                if let Happened::Changed { change: SessionChange::Composer { composer }, .. } = event.happened
                                    && sender.send(Ok(*composer)).is_err() { return; }
                            }
                            // A composer has no history; nothing of one is asked for.
                            Ok(Some(Watched::History { .. } | Watched::Unchanged { .. } | Watched::Segments { .. } | Watched::Now { .. })) => {},
                            Ok(Some(Watched::Documents { .. })) => {
                                let _ = sender.send(Err(Error::Unexpected("document packets arrived on a legacy composer watch".into())));
                                return;
                            }
                            Ok(None) => return,
                            Err(error) => { let _ = sender.send(Err(error.into())); return; }
                        }
                    }
                }
            }
        });
        let mut composing = Composing { states, revision: None };
        let state = composing
            .next()
            .await?
            .ok_or_else(|| Error::Unexpected("composer watch ended before its snapshot".into()))?;
        Ok((composing, state))
    }
}

/// Cancellation-safe snapshot stream. Dropping it closes its read task and watch.
pub struct Composing {
    states: watch_queue::Receiver<Result<Composer, Error>>,
    revision: Option<u64>,
}

impl Composing {
    pub async fn next(&mut self) -> Result<Option<Composer>, Error> {
        while let Some(state) = self.states.recv().await.map_err(|error| Error::Unexpected(error.to_string()))? {
            let state = state?;
            if self.revision.is_some_and(|revision| state.revision <= revision) {
                continue;
            }
            self.revision = Some(state.revision);
            return Ok(Some(state));
        }
        Ok(None)
    }
}
