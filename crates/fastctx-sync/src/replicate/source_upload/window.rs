//! Independent batches retain their state until ordered receipts commit.
use super::*;
use crate::{store::sources::uploads::UploadBatch, transfer::Message};
use tokio::task::JoinSet;

impl SourceUploader {
    async fn prepare_content(
        &self,
        upstream: Arc<dyn Upstream>,
        mut state: SourceUploadState,
        batch: UploadBatch,
    ) -> Result<(SourceUploadState, UploadBatch), Error> {
        while state.phase == SourceUploadPhase::Content {
            let budget = self.store.budget();
            let available = budget.capacity().saturating_sub(budget.reserved());
            let slots = (available / (fastctx_types::sync::BLOCK_BYTES as usize + 65536)).max(1);
            let nodes = self.store.next_nodes(&state, slots).await?;
            if nodes.is_empty() {
                self.store.ready(&state).await?;
                state.phase = SourceUploadPhase::Submit;
                break;
            }
            let mut work = JoinSet::new();
            for node in nodes {
                let permit = match budget.try_acquire(16384) {
                    Ok(permit) => permit,
                    Err(Error::Busy) => break,
                    Err(e) => return Err(e),
                };
                let driver = self.clone();
                let upstream = upstream.clone();
                let state = state.clone();
                let upload = batch.transfer.clone();
                work.spawn(async move {
                    let _permit = permit;
                    Box::pin(driver.content(upstream.as_ref(), &state, &upload, node)).await
                });
            }
            if work.is_empty() {
                return Err(Error::Busy);
            }
            let mut busy = false;
            while let Some(result) = work.join_next().await {
                match result.map_err(|e| Error::Unavailable(format!("source content worker stopped: {e}")))? {
                    Ok(()) => {}
                    Err(Error::Busy) => busy = true,
                    Err(error) => return Err(error),
                }
            }
            if busy {
                return Err(Error::Busy);
            }
        }
        Ok((state, batch))
    }
    async fn submit(
        &self,
        upstream: Arc<dyn Upstream>,
        state: SourceUploadState,
        batch: UploadBatch,
        previous: Option<tokio::sync::oneshot::Receiver<()>>,
        sent: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(), Error> {
        if let Some(previous) = previous {
            previous.await.map_err(|_| Error::Unavailable("earlier source dispatch stopped".into()))?;
        }
        self.store.check(&state).await?;
        let mut request =
            upstream.exchange(Message::Source { holding: state.holding.clone(), batch: batch.signed.clone() });
        let mut sent = Some(sent);
        // The next batch may start as soon as this request has entered the
        // transport, even while its acknowledgement is still outstanding.
        let response = std::future::poll_fn(|cx| {
            let result = request.as_mut().poll(cx);
            if let Some(sent) = sent.take() {
                let _ = sent.send(());
            }
            result
        });
        let reply = tokio::time::timeout(self.timeout, response).await.map_err(|_| {
            Error::Unavailable("source submission timed out; its original proof remains pending".into())
        })??;
        self.store.check(&state).await?;
        match reply.value() {
            Message::SourceResult {
                result: crate::store::authority::SourceAccepted::Accepted { receipt, content_complete },
            } => self.store.acknowledge(&state, &batch, *receipt.clone(), *content_complete).await,
            Message::SourceResult { result: crate::store::authority::SourceAccepted::AwaitingEpoch { proof } } => {
                self.store.waiting(&state, proof).await
            }
            Message::Revoked { holding } if holding == &state.holding => Err(Error::Revoked),
            _ => Err(Error::Invalid("hub answered source submission with another message".into())),
        }
    }
    pub(super) async fn advance(
        &self,
        upstream: Arc<dyn Upstream>,
        state: &SourceUploadState,
    ) -> Result<SourceUploadProgress, Error> {
        self.store.prune(state).await?;
        let budget = self.store.budget();
        let mut after = state.through;
        let mut window = Vec::new();
        loop {
            if !window.is_empty() && budget.reserved() >= budget.capacity() / 2 {
                break;
            }
            let next = self.store.at(state, after).await?;
            let batch = match self.store.batch(&next).await {
                Ok(Some(batch)) => batch,
                Ok(None) => break,
                Err(Error::Busy) if !window.is_empty() => break,
                Err(e) => return Err(e),
            };
            if next.proof.is_none() {
                self.store.start(&next, &batch).await?;
            }
            let next = self.store.at(state, after).await?;
            after = batch.batch.through;
            window.push((next, batch));
        }
        if window.is_empty() {
            return Ok(SourceUploadProgress::Idle(state.clone()));
        }
        let mut preparing = JoinSet::new();
        let mut ready = Vec::new();
        for (state, batch) in window {
            if state.phase == SourceUploadPhase::Content {
                let driver = self.clone();
                let upstream = upstream.clone();
                preparing.spawn(async move { Box::pin(driver.prepare_content(upstream, state, batch)).await });
            } else {
                ready.push((state, batch));
            }
        }
        let mut transient = None;
        while let Some(result) = preparing.join_next().await {
            match result.map_err(|e| Error::Unavailable(format!("source batch worker stopped: {e}")))? {
                Ok(pair) => ready.push(pair),
                Err(Error::Busy) => transient = Some(Error::Busy),
                Err(Error::Unavailable(message)) => transient = Some(Error::Unavailable(message)),
                Err(e) => return Err(e),
            }
        }
        ready.sort_by_key(|(state, _)| state.through);
        let mut sending = JoinSet::new();
        let mut previous = None;
        let mut contiguous = state.through;
        for (state, batch) in ready {
            if state.through != contiguous {
                break;
            }
            contiguous = batch.batch.through;
            if state.phase == SourceUploadPhase::Confirm {
                continue;
            }
            let (sent, waiting) = tokio::sync::oneshot::channel();
            let driver = self.clone();
            let upstream = upstream.clone();
            let prior = previous.take();
            sending.spawn(async move { Box::pin(driver.submit(upstream, state, batch, prior, sent)).await });
            previous = Some(waiting);
        }
        while let Some(result) = sending.join_next().await {
            match result.map_err(|e| Error::Unavailable(format!("source submission worker stopped: {e}")))? {
                Ok(()) => {}
                Err(Error::Busy) => transient = Some(Error::Busy),
                Err(Error::Unavailable(message)) => transient = Some(Error::Unavailable(message)),
                Err(e) => return Err(e),
            }
        }
        let mut accepted = false;
        loop {
            let current = self
                .store
                .load(state.key.clone())
                .await?
                .ok_or_else(|| Error::Corrupt("upload cursor disappeared".into()))?;
            if current.stopped.is_some() {
                return Ok(SourceUploadProgress::Stopped(current));
            }
            if current.phase != SourceUploadPhase::Confirm {
                if current.through == state.through
                    && let Some(error) = transient
                {
                    return Err(error);
                }
                return Ok(match current.phase {
                    SourceUploadPhase::Waiting if current.receipt.is_some() => {
                        SourceUploadProgress::AwaitingContent(current)
                    }
                    SourceUploadPhase::Waiting => SourceUploadProgress::AwaitingPredecessor(current),
                    _ if accepted => SourceUploadProgress::Accepted(current),
                    _ => SourceUploadProgress::Advanced(current),
                });
            }
            let batch = self
                .store
                .batch(&current)
                .await?
                .ok_or_else(|| Error::Corrupt("acknowledged source proof disappeared".into()))?;
            self.store.finish(&current, &batch).await?;
            accepted = true;
        }
    }
}
