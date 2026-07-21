//! Request-scoped cancellation and retirement state for file operations.

use crate::model::ToolResponse;
use rmcp::model::RequestId;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

struct CancelOnDropState {
    cancel: CancellationToken,
    armed: AtomicBool,
}

/// Cancels the blocking sibling whenever its async request future is dropped.
#[must_use]
pub(crate) struct RequestWorkGuard {
    state: Arc<CancelOnDropState>,
}

impl RequestWorkGuard {
    /// Creates a child cancellation scope shared with the blocking operation.
    pub(crate) fn new(
        request_id: RequestId,
        request_cancel: CancellationToken,
    ) -> (Self, OperationCtx) {
        Self::new_with_state(request_id, request_cancel, None)
    }

    fn new_with_state(
        request_id: RequestId,
        request_cancel: CancellationToken,
        #[cfg(test)] stage_hook: Option<TestStageHook>,
        #[cfg(not(test))] _stage_hook: Option<()>,
    ) -> (Self, OperationCtx) {
        let state = Arc::new(CancelOnDropState {
            cancel: request_cancel.child_token(),
            armed: AtomicBool::new(true),
        });
        let operation = OperationCtx {
            request_id,
            state: Arc::clone(&state),
            #[cfg(test)]
            stage_hook,
        };
        (Self { state }, operation)
    }

    #[cfg(test)]
    pub(crate) fn new_with_hook(
        request_id: RequestId,
        request_cancel: CancellationToken,
        stage_hook: TestStageHook,
    ) -> (Self, OperationCtx) {
        Self::new_with_state(request_id, request_cancel, Some(stage_hook))
    }

    /// Marks a normally joined blocking sibling so dropping this guard is inert.
    pub(crate) fn disarm(&mut self) {
        self.state.armed.store(false, Ordering::Release);
    }
}

impl Drop for RequestWorkGuard {
    fn drop(&mut self) {
        if self.state.armed.swap(false, Ordering::AcqRel) {
            self.state.cancel.cancel();
        }
    }
}

/// Request-wide cancellation state cloned into coordinators and worker contexts.
#[derive(Clone)]
pub(crate) struct OperationCtx {
    request_id: RequestId,
    state: Arc<CancelOnDropState>,
    #[cfg(test)]
    stage_hook: Option<TestStageHook>,
}

impl OperationCtx {
    /// Returns an error after the MCP request or its async wrapper is cancelled.
    pub(crate) fn check(&self) -> Result<(), OpError> {
        if self.state.cancel.is_cancelled() {
            Err(OpError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Returns the token used by cancel-aware async admission.
    pub(crate) fn cancellation_token(&self) -> &CancellationToken {
        &self.state.cancel
    }

    /// Creates request-only work state for coordinator and inline work.
    pub(crate) fn inline_work(&self) -> WorkCtx {
        WorkCtx {
            request: self.clone(),
            epoch: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn stage(&self, stage: TestStage) {
        if let Some(hook) = &self.stage_hook {
            hook(stage);
        }
    }
}

impl fmt::Debug for OperationCtx {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationCtx")
            .field("request_id", &self.request_id)
            .field("cancelled", &self.state.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Generation-aware cancellation state for inline or speculative work.
#[derive(Clone, Debug)]
pub(crate) struct WorkCtx {
    request: OperationCtx,
    epoch: Option<EpochGuard>,
}

impl WorkCtx {
    /// Creates speculative work that must stop when `epoch` is retired.
    pub(crate) fn speculative(request: OperationCtx, epoch: EpochGuard) -> Self {
        Self {
            request,
            epoch: Some(epoch),
        }
    }

    /// Checks request cancellation before speculative generation retirement.
    pub(crate) fn check(&self) -> Result<(), WorkStop> {
        if self.request.state.cancel.is_cancelled() {
            return Err(WorkStop::RequestCancelled);
        }
        if let Some(epoch) = &self.epoch
            && epoch.current.load(Ordering::Acquire) != epoch.expected
        {
            return Err(WorkStop::EpochRetired);
        }
        Ok(())
    }

    /// Maps request-only work to the user-visible operation error channel.
    pub(crate) fn check_inline(&self) -> Result<(), OpError> {
        debug_assert!(self.epoch.is_none());
        match self.check() {
            Ok(()) => Ok(()),
            Err(WorkStop::RequestCancelled) => Err(OpError::Cancelled),
            Err(WorkStop::EpochRetired) => {
                unreachable!("inline work never carries an epoch guard")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn stage(&self, stage: TestStage) {
        self.request.stage(stage);
    }
}

/// Snapshot of the speculative generation a worker is allowed to publish into.
#[derive(Clone, Debug)]
pub(crate) struct EpochGuard {
    expected: u64,
    current: Arc<AtomicU64>,
}

impl EpochGuard {
    /// Captures one published generation for a speculative worker.
    pub(crate) fn new(expected: u64, current: Arc<AtomicU64>) -> Self {
        Self { expected, current }
    }
}

/// Cooperative reasons a worker must stop without publishing an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkStop {
    RequestCancelled,
    EpochRetired,
}

/// A cancellation/retirement checkpoint accepted by deep file-search loops.
pub(crate) trait WorkCheckpoint: Send + Sync {
    /// Checks request cancellation before any speculative generation retirement.
    fn check_work(&self) -> Result<(), WorkStop>;

    #[cfg(test)]
    fn stage(&self, stage: TestStage);
}

impl WorkCheckpoint for OperationCtx {
    fn check_work(&self) -> Result<(), WorkStop> {
        self.check()
            .map_err(|OpError::Cancelled| WorkStop::RequestCancelled)
    }

    #[cfg(test)]
    fn stage(&self, stage: TestStage) {
        OperationCtx::stage(self, stage);
    }
}

impl WorkCheckpoint for WorkCtx {
    fn check_work(&self) -> Result<(), WorkStop> {
        self.check()
    }

    #[cfg(test)]
    fn stage(&self, stage: TestStage) {
        WorkCtx::stage(self, stage);
    }
}

/// Errors returned through the new grep/glob operation spine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpError {
    Cancelled,
}

impl OpError {
    /// Converts operation control flow without disguising cancellation as success.
    pub(crate) fn into_response(self) -> ToolResponse {
        match self {
            Self::Cancelled => ToolResponse::error("Request cancelled."),
        }
    }
}

#[cfg(test)]
pub(crate) type TestStageHook = Arc<dyn Fn(TestStage) + Send + Sync + 'static>;

/// Deterministic barriers and fault points used by cancellation ownership tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TestStage {
    BeforeFilePermit,
    AfterFilePermit,
    RunnerQueued,
    RunnerStarted,
    BeforeBurstAcquire,
    BeforeCandidateClaim,
    AfterCandidateClaim,
    BeforeReadyPublish,
    TraversalEntry,
    TraversalBatchFlush,
    CapturePreflightRead,
    SnapshotChunk,
    SnapshotPromote,
    BeforeIdentityPostCheck,
    EncodingChunk,
    LegacySegment,
    CandidateValidation,
    BeforeRegexSearch,
    SinkMatch,
    OccurrenceBatch,
    OrderedReduce,
    SortChunk,
    SortMerge,
    RenderUnit,
    TokenProbe,
    BeforeFinalTokenVerify,
}

#[cfg(test)]
mod tests {
    use super::{EpochGuard, OpError, RequestWorkGuard, TestStage, WorkCtx, WorkStop};
    use rmcp::model::RequestId;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio_util::sync::CancellationToken;

    fn request_id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    #[test]
    fn dropping_an_armed_guard_cancels_only_its_child_scope() {
        let parent = CancellationToken::new();
        let (guard, operation) = RequestWorkGuard::new(request_id(1), parent.clone());
        assert_eq!(operation.check(), Ok(()));
        drop(guard);
        assert_eq!(operation.check(), Err(OpError::Cancelled));
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn disarming_a_joined_guard_does_not_cancel_the_operation_scope() {
        let parent = CancellationToken::new();
        let (mut guard, operation) = RequestWorkGuard::new(request_id(2), parent);
        guard.disarm();
        drop(guard);
        assert_eq!(operation.check(), Ok(()));
    }

    #[test]
    fn work_checks_request_cancellation_before_epoch_retirement() {
        let parent = CancellationToken::new();
        let (_guard, operation) = RequestWorkGuard::new(request_id(3), parent.clone());
        let generation = Arc::new(AtomicU64::new(7));
        let work = WorkCtx {
            request: operation,
            epoch: Some(EpochGuard {
                expected: 7,
                current: Arc::clone(&generation),
            }),
        };
        assert_eq!(work.check(), Ok(()));
        generation.store(8, Ordering::Release);
        assert_eq!(work.check(), Err(WorkStop::EpochRetired));
        parent.cancel();
        assert_eq!(work.check(), Err(WorkStop::RequestCancelled));
    }

    #[test]
    fn stage_inventory_is_unique_and_complete() {
        let stages = [
            TestStage::BeforeFilePermit,
            TestStage::AfterFilePermit,
            TestStage::RunnerQueued,
            TestStage::RunnerStarted,
            TestStage::BeforeBurstAcquire,
            TestStage::BeforeCandidateClaim,
            TestStage::AfterCandidateClaim,
            TestStage::BeforeReadyPublish,
            TestStage::TraversalEntry,
            TestStage::TraversalBatchFlush,
            TestStage::CapturePreflightRead,
            TestStage::SnapshotChunk,
            TestStage::SnapshotPromote,
            TestStage::BeforeIdentityPostCheck,
            TestStage::EncodingChunk,
            TestStage::LegacySegment,
            TestStage::CandidateValidation,
            TestStage::BeforeRegexSearch,
            TestStage::SinkMatch,
            TestStage::OccurrenceBatch,
            TestStage::OrderedReduce,
            TestStage::SortChunk,
            TestStage::SortMerge,
            TestStage::RenderUnit,
            TestStage::TokenProbe,
            TestStage::BeforeFinalTokenVerify,
        ];
        assert_eq!(stages.len(), 26);
        assert_eq!(stages.into_iter().collect::<HashSet<_>>().len(), 26);
    }
}
