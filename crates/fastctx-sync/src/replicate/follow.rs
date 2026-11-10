//! A bounded network driver over durable receiver state. Transport failure leaves
//! the last verified commit intact; only a remote permission denial stops a scope.
use super::{DirectoryFollower, Upstream};
use crate::{
    cursor::{ScopeCursor, Sequence},
    identity::{ScopeId, ScopeKey},
    proof::EpochLookup,
    registry::REGISTRY,
    scope::{DiscoveryCursor, ScopeOffer},
    store::{
        Error,
        ingest::StreamProgress,
        replica::{FollowScope, FollowStop, ReplicaContext, ReplicaSink, SqliteReplica, StreamInstalled},
    },
    timeline::{Direction, StreamPosition, StreamRead},
    transfer::{KindVersion, Message, ViewDefinition},
};

#[derive(Clone, Debug)]
pub enum FollowProgress {
    /// Feed and source positions are current; this never acknowledges body blocks.
    CaughtUp,
    Progress,
    Reset,
    Stopped(FollowStop),
}
pub struct FollowPage {
    pub scopes: Vec<(ScopeKey, Result<FollowProgress, Error>)>,
    pub next: Option<ScopeId>,
}
pub struct FollowDiscovery {
    pub scopes: Vec<ScopeKey>,
    pub next: Option<DiscoveryCursor>,
}

#[derive(Clone)]
pub struct ReplicaFollower {
    replica: SqliteReplica,
    max_records: u32,
    max_bytes: u32,
    kinds: Option<std::collections::BTreeSet<KindVersion>>,
    request_timeout: std::time::Duration,
}
impl ReplicaFollower {
    pub fn new(replica: SqliteReplica) -> Self {
        Self {
            replica,
            max_records: 100,
            max_bytes: fastctx_types::sync::PAGE_BYTES,
            kinds: None,
            request_timeout: std::time::Duration::from_secs(30),
        }
    }
    /// Restricts new enrollments to the assembly's supported kinds. Existing
    /// views remain durable and need an explicit view transition to change.
    pub fn with_kinds(mut self, kinds: Vec<KindVersion>) -> Result<Self, Error> {
        let mut selected = std::collections::BTreeSet::new();
        for kind in kinds {
            let registered = REGISTRY.asset(&kind.kind).map_err(Error::Invalid)?;
            if registered.registration().schema != kind.schema || !selected.insert(kind) {
                return Err(Error::Invalid("following kinds must be distinct supported schemas".into()));
            }
        }
        self.kinds = Some(selected);
        Ok(self)
    }
    /// A hung transport request must release this pass for other scopes.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Result<Self, Error> {
        if timeout.is_zero() {
            return Err(Error::Invalid("following timeout must be positive".into()));
        }
        self.request_timeout = timeout;
        Ok(self)
    }
    pub fn with_limits(mut self, max_records: u32, max_bytes: u32) -> Result<Self, Error> {
        if max_records == 0 || max_records > 256 || max_bytes == 0 || max_bytes > fastctx_types::sync::PAGE_BYTES {
            return Err(Error::Invalid("following limits exceed one metadata page".into()));
        }
        self.max_records = max_records;
        self.max_bytes = max_bytes;
        Ok(self)
    }
    /// Installs and enrolls at most one discovery page. Repeating a page keeps
    /// existing progress; discovery's nonzero cursors never skip local history.
    pub async fn discover_page(
        &self,
        directory: &DirectoryFollower,
        upstream: &dyn Upstream,
        cursor: Option<DiscoveryCursor>,
    ) -> Result<FollowDiscovery, Error> {
        let page = directory.page(upstream, cursor).await?;
        let mut scopes = Vec::new();
        for (offer, state) in page.installed {
            let enabled = match state {
                crate::store::scopes::Installed::Opened(s)
                | crate::store::scopes::Installed::Converted(s)
                | crate::store::scopes::Installed::Advanced(s)
                | crate::store::scopes::Installed::Unchanged(s) => s.enabled,
            };
            let scope = offer.cursor.scope.clone();
            if enabled {
                self.track(offer).await?;
            } else if let Some(tracked) = self.replica.follow_scope(scope.clone()).await? {
                self.replica.stop_follow(tracked, FollowStop::Revoked).await?;
            }
            scopes.push(scope);
        }
        Ok(FollowDiscovery { scopes, next: page.next })
    }
    /// `offer` comes from authenticated discovery after its description installed.
    /// Only ordinary replica custody is requested by this driver.
    pub async fn track(&self, offer: ScopeOffer) -> Result<(), Error> {
        let scope = offer.cursor.scope.clone();
        if let Some(known) = self.replica.follow_scope(scope.clone()).await? {
            if known.stopped.is_some() {
                return Ok(());
            }
            let current = self.replica.follow_holding(scope).await?;
            if Some(&known.holding) != current.as_ref() {
                self.replica.stop_follow(known, FollowStop::HoldingChanged).await?;
            }
            return Ok(());
        }
        let holding = self.replica.follow_holding(scope.clone()).await?.ok_or(Error::Revoked)?;
        if holding.acl_revision != offer.cursor.acl_revision {
            return Err(Error::Busy);
        }
        let kinds = offer
            .kinds
            .into_iter()
            .filter(|k| {
                REGISTRY.asset(&k.kind).is_ok_and(|registered| registered.registration().schema == k.schema)
                    && self.kinds.as_ref().is_none_or(|selected| selected.contains(k))
            })
            .collect();
        let view =
            ViewDefinition { scope: scope.clone(), acl_revision: holding.acl_revision, projection_schema: 1, kinds };
        let cursor = ScopeCursor {
            through: Sequence(0),
            view: Some(view.id().map_err(Error::Invalid)?),
            prefix_digest: Some(fastctx_types::sync::feed::genesis(&scope, &offer.cursor.epoch)),
            ..offer.cursor
        };
        let context =
            ReplicaContext { source: crate::identity::Producer::Hub, holding, source_uses: Default::default() };
        self.replica.begin(context, cursor, view).await?;
        Ok(())
    }
    /// Each scope gets one change page and at most one epoch/range request. The
    /// caller schedules later passes; a busy scope cannot monopolize this pass.
    pub async fn poll(&self, upstream: &dyn Upstream, after: Option<ScopeId>, max: u32) -> Result<FollowPage, Error> {
        let states = self.replica.follow_scopes(after, max).await?;
        let next =
            if states.len() == max as usize { states.last().map(|s| s.holding.scope.scope.clone()) } else { None };
        let mut scopes = Vec::new();
        for state in states {
            let key = state.holding.scope.clone();
            scopes.push((key, Box::pin(self.advance(upstream, state)).await));
        }
        Ok(FollowPage { scopes, next })
    }
    pub async fn step(&self, upstream: &dyn Upstream, scope: ScopeKey) -> Result<FollowProgress, Error> {
        let state =
            self.replica.follow_scope(scope).await?.ok_or_else(|| Error::Unavailable("scope is not tracked".into()))?;
        Box::pin(self.advance(upstream, state)).await
    }
    async fn advance(&self, upstream: &dyn Upstream, state: FollowScope) -> Result<FollowProgress, Error> {
        if let Some(reason) = &state.stopped {
            return Ok(FollowProgress::Stopped(reason.clone()));
        }
        if self.replica.follow_holding(state.holding.scope.clone()).await?.as_ref() != Some(&state.holding) {
            self.replica.stop_follow(state, FollowStop::HoldingChanged).await?;
            return Ok(FollowProgress::Stopped(FollowStop::HoldingChanged));
        }
        self.replica.retire_follow_pages(state.clone()).await?;
        let answer = self
            .exchange(
                upstream,
                &state,
                Message::Changes {
                    holding: state.holding.clone(),
                    after: state.cursor.clone(),
                    view: Some(state.view.clone()),
                    max_bytes: self.max_bytes,
                },
            )
            .await?;
        let Some(answer) = answer else {
            return self.stopped(&state).await;
        };
        let (changed, more) = match answer {
            Message::Reset { scope, epoch, .. } if scope == state.holding.scope => {
                self.replica.restart_follow(state, epoch).await?;
                return Ok(FollowProgress::Reset);
            }
            Message::ChangesPage { holding, page } if holding == state.holding && page.after == state.cursor => {
                let changed = page.through != page.after;
                let more = page.more;
                if !changed && (more || !page.entries.is_empty() || !page.proofs.is_empty()) {
                    return Err(Error::Invalid("the feed page made no progress".into()));
                }
                if changed {
                    self.replica.apply_changes(state.context(), page).await?;
                }
                (changed, more)
            }
            _ => return Err(Error::Invalid("the hub answered changes with another scope, holding or message".into())),
        };
        let Some(need) = self.replica.next_stream(state.context()).await? else {
            return Ok(if more || changed { FollowProgress::Progress } else { FollowProgress::CaughtUp });
        };
        let target = self.replica.stream_target(state.context(), need.stream.clone(), need.epoch_proof.clone()).await?;
        if target.received.is_none() {
            return self.get_epoch(upstream, &state, EpochLookup::Proof { id: need.epoch_proof }).await;
        }
        if let Some(epoch) = self.replica.missing_epochs(state.context(), need.stream.clone()).await?.into_iter().next()
        {
            return self.get_epoch(upstream, &state, EpochLookup::Stream { stream: need.stream, epoch }).await;
        }
        if let Some(installed) =
            self.replica.resume_stream(state.context(), need.stream.clone(), need.epoch_proof.clone()).await?
        {
            return match installed {
                StreamInstalled::Committed { .. } => Ok(FollowProgress::Progress),
                StreamInstalled::AwaitingEpoch { proof } => {
                    self.get_epoch(upstream, &state, EpochLookup::Proof { id: proof }).await
                }
            };
        }
        let Some(StreamProgress::Confirmed { cursor, .. }) = target.received else {
            return Err(Error::Invalid("a replica target must be confirmed before reading ranges".into()));
        };
        if cursor.committed >= target.through {
            self.replica.finish_stream(state.context(), need).await?;
            return Ok(FollowProgress::Progress);
        }
        let answer = self
            .exchange(
                upstream,
                &state,
                Message::StreamRange {
                    holding: state.holding.clone(),
                    request: StreamRead {
                        position: StreamPosition::After(cursor.committed),
                        stream: cursor.clone(),
                        through: Some(target.through),
                        max_records: self.max_records,
                        max_bytes: self.max_bytes,
                    },
                },
            )
            .await?;
        let Some(answer) = answer else {
            return self.stopped(&state).await;
        };
        let Message::StreamPage { holding, page } = answer else {
            return Err(Error::Invalid("the hub did not answer the stream range".into()));
        };
        if holding != state.holding
            || page.records.len() > self.max_records as usize
            || page.direction != Direction::Forward
            || page.records.first().map(|r| r.record.sequence.0) != cursor.committed.0.checked_add(1)
            || !matches!(&page.fence, StreamProgress::Confirmed { cursor: fence, epoch_proof, .. }
                if fence.scope==cursor.scope && fence.stream==cursor.stream && fence.epoch==cursor.epoch
                && fence.committed==target.through && epoch_proof==&need.epoch_proof)
        {
            return Err(Error::Invalid("stream response differs from the requested target".into()));
        }
        self.replica.apply_stream(state.context(), page).await?;
        self.replica.finish_stream(state.context(), need).await?;
        Ok(FollowProgress::Progress)
    }
    async fn get_epoch(
        &self,
        upstream: &dyn Upstream,
        state: &FollowScope,
        lookup: EpochLookup,
    ) -> Result<FollowProgress, Error> {
        let answer = self
            .exchange(upstream, state, Message::GetEpoch { scope: state.holding.scope.clone(), lookup: lookup.clone() })
            .await?;
        let Some(answer) = answer else {
            return self.stopped(state).await;
        };
        let Message::Epoch { descriptor } = answer else {
            return Err(Error::Invalid("the hub did not answer the epoch lookup".into()));
        };
        let decoded: crate::proof::EpochDescriptor =
            serde_json::from_slice(&descriptor.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
        let matches = match lookup {
            EpochLookup::Proof { id } => descriptor.proof.id() == id,
            EpochLookup::Stream { stream, epoch } => decoded.stream == stream && decoded.epoch == epoch,
        };
        if !matches || decoded.scope != state.holding.scope {
            return Err(Error::Invalid("epoch response differs from its request".into()));
        }
        self.replica.install_epoch(state.context(), descriptor).await?;
        Ok(FollowProgress::Progress)
    }
    async fn exchange(
        &self,
        upstream: &dyn Upstream,
        state: &FollowScope,
        request: Message,
    ) -> Result<Option<Message>, Error> {
        let max = match &request {
            Message::Changes { max_bytes, .. } => *max_bytes,
            Message::StreamRange { request, .. } => request.max_bytes,
            _ => fastctx_types::sync::PAGE_BYTES,
        };
        let answer = tokio::time::timeout(self.request_timeout, upstream.exchange(request))
            .await
            .map_err(|_| Error::Unavailable("replica upstream request timed out".into()))?;
        let reason = match &answer {
            Err(Error::Denied(message)) => Some(FollowStop::Denied { message: message.clone() }),
            Err(Error::Revoked) => Some(FollowStop::Revoked),
            Ok(Message::Revoked { holding }) if holding == &state.holding => Some(FollowStop::Revoked),
            _ => None,
        };
        if let Some(reason) = reason {
            self.replica.stop_follow(state.clone(), reason).await?;
            return Ok(None);
        }
        let answer = answer?;
        if serde_json::to_vec(&answer).map_err(|e| Error::Invalid(e.to_string()))?.len() > max as usize {
            return Err(Error::Invalid("the hub response exceeds the requested page budget".into()));
        }
        Ok(Some(answer))
    }
    async fn stopped(&self, state: &FollowScope) -> Result<FollowProgress, Error> {
        let current = self.replica.follow_scope(state.holding.scope.clone()).await?.ok_or(Error::Busy)?;
        current.stopped.map(FollowProgress::Stopped).ok_or(Error::Busy)
    }
}
