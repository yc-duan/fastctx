//! Resource-admitted windows pipeline content and original source proofs.
mod content;
mod window;
use super::Upstream;
pub use crate::store::sources::{SourceUploadKey, SourceUploadPhase, SourceUploadState, SourceUploadStop};
use crate::{
    identity::HubId,
    store::{
        Error,
        sources::{SqliteSources, uploads::SourceUploads},
    },
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum SourceUploadProgress {
    NotPromoted { key: SourceUploadKey },
    Idle(SourceUploadState),
    Advanced(SourceUploadState),
    AwaitingPredecessor(SourceUploadState),
    AwaitingContent(SourceUploadState),
    Accepted(SourceUploadState),
    Stopped(SourceUploadState),
}
#[derive(Clone, Debug)]
pub struct SourceUploadPage {
    pub progress: Vec<(SourceUploadKey, Result<SourceUploadProgress, Error>)>,
    pub next: Option<SourceUploadKey>,
}
#[derive(Clone)]
pub struct SourceUploader {
    store: SourceUploads,
    timeout: std::time::Duration,
    window: Arc<tokio::sync::Mutex<()>>,
}
impl SourceUploader {
    pub fn new(sources: SqliteSources, hub: HubId) -> Result<Self, Error> {
        Ok(Self {
            store: SourceUploads::new(sources, hub)?,
            timeout: std::time::Duration::from_secs(30),
            window: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Result<Self, Error> {
        if timeout.is_zero() {
            return Err(Error::Invalid("source upload timeout must be positive".into()));
        }
        self.timeout = timeout;
        Ok(self)
    }
    /// Diagnostic state remains available after local or remote revocation.
    pub async fn status(&self, key: SourceUploadKey) -> Result<Option<SourceUploadState>, Error> {
        self.store.status(key).await
    }
    /// Advances one resource-admitted window per stream, in keyset order. A
    /// stream's temporary failure does not prevent other streams from progressing.
    pub async fn poll(
        &self,
        upstream: Arc<dyn Upstream>,
        after: Option<SourceUploadKey>,
        max: u32,
    ) -> Result<SourceUploadPage, Error> {
        let keys = self.store.keys(after, max).await?;
        let next = if keys.len() == max as usize { keys.last().cloned() } else { None };
        let mut progress = Vec::with_capacity(keys.len());
        for key in keys {
            let result = self.step(upstream.clone(), key.clone()).await;
            progress.push((key, result));
        }
        Ok(SourceUploadPage { progress, next })
    }
    /// Pipelines multiple offers and Source requests without waiting for each
    /// preceding acknowledgement. Shared byte admission determines the window;
    /// there is no transfer-rate limit. Receipts advance only a contiguous prefix.
    /// An interrupted window replays the same durable proofs and operations.
    pub async fn step(&self, upstream: Arc<dyn Upstream>, key: SourceUploadKey) -> Result<SourceUploadProgress, Error> {
        let _window = self.window.lock().await;
        let Some(state) = self.store.load(key.clone()).await? else {
            return Ok(SourceUploadProgress::NotPromoted { key });
        };
        if state.stopped.is_some() {
            return Ok(SourceUploadProgress::Stopped(state));
        }
        match Box::pin(self.advance(upstream, &state)).await {
            Ok(result) => Ok(result),
            Err(error) => {
                let reason = match &error {
                    Error::Revoked => Some(SourceUploadStop::Revoked),
                    Error::Denied(message) => Some(SourceUploadStop::Denied { message: message.clone() }),
                    Error::Invalid(message) => Some(SourceUploadStop::Rejected { message: message.clone() }),
                    Error::Corrupt(message) => Some(SourceUploadStop::Corrupt { message: message.clone() }),
                    _ => None,
                };
                if let Some(reason) = reason {
                    self.store.stop(&state, reason).await?;
                    if let Some(stopped) = self.store.status(state.key.clone()).await?
                        && stopped.stopped.is_some()
                    {
                        return Ok(SourceUploadProgress::Stopped(stopped));
                    }
                }
                Err(error)
            }
        }
    }
}
