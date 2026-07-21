//! Cancelable fixed-chunk sorting on the shared grep/glob extra executor.

use crate::file_executor::{BurstEnvelope, BurstPermit, BurstUse, GrepGlobExecutor};
use crate::operation::OperationCtx;
#[cfg(test)]
use crate::operation::TestStage;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

pub(crate) const SORT_CHUNK_ITEMS: usize = 4_096;
const MERGE_CANCEL_ITEMS: usize = 1_024;
const LEAF_WAIT_POLL: Duration = Duration::from_millis(10);

/// A sorted collection plus test-only evidence about bounded execution.
pub(crate) struct SortOutput<T> {
    pub(crate) items: Vec<T>,
    #[cfg(test)]
    pub(crate) metrics: SortMetrics,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SortMetrics {
    pub(crate) chunks: usize,
    pub(crate) inline_chunks: usize,
    pub(crate) spawned_chunks: usize,
    pub(crate) merge_pops: usize,
}

/// Failures that must stop the request instead of returning a partial order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SortError {
    Cancelled,
    WorkerPanicked,
    WorkerDisconnected,
    Invariant,
}

impl fmt::Display for SortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Request cancelled."),
            Self::WorkerPanicked => formatter
                .write_str("Internal file sorting failure: a bounded sort worker panicked."),
            Self::WorkerDisconnected => formatter.write_str(
                "Internal file sorting failure: a bounded sort worker exited without a result.",
            ),
            Self::Invariant => formatter.write_str(
                "Internal file sorting failure: the bounded sort state was inconsistent.",
            ),
        }
    }
}

/// Sorts fixed chunks with try-only extra leaves, then performs a deterministic
/// cancelable k-way merge on the coordinator.
pub(crate) fn sort_cancelable<T, F>(
    items: Vec<T>,
    compare: F,
    operation: Option<&OperationCtx>,
    executor: Option<&Arc<GrepGlobExecutor>>,
) -> Result<SortOutput<T>, SortError>
where
    T: Send + 'static,
    F: Fn(&T, &T) -> Ordering + Send + Sync + 'static,
{
    check_operation(operation)?;
    if items.len() <= SORT_CHUNK_ITEMS {
        let mut items = items;
        sort_inline_chunk(&mut items, &compare, operation)?;
        return Ok(SortOutput {
            items,
            #[cfg(test)]
            metrics: SortMetrics {
                chunks: 1,
                inline_chunks: 1,
                ..SortMetrics::default()
            },
        });
    }

    let mut source = items.into_iter();
    let mut chunks = Vec::new();
    loop {
        let chunk = source.by_ref().take(SORT_CHUNK_ITEMS).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);
    }
    let chunk_count = chunks.len();
    let compare = Arc::new(compare);
    let (sender, receiver) = mpsc::channel::<Result<(usize, BurstEnvelope<Vec<T>>), SortError>>();
    let mut inline_chunks = Vec::new();
    let mut chunks = chunks.into_iter().enumerate();
    let Some(base_chunk) = chunks.next() else {
        return Err(SortError::Invariant);
    };
    inline_chunks.push(base_chunk);
    let mut spawned = 0_usize;

    for (chunk_index, chunk) in chunks {
        check_operation(operation)?;
        let Some(executor) = executor else {
            inline_chunks.push((chunk_index, chunk));
            continue;
        };
        let Some(ticket) = executor.try_runner_ticket() else {
            inline_chunks.push((chunk_index, chunk));
            continue;
        };
        let Some(permit) = executor.try_burst(BurstUse::SortExtra) else {
            drop(ticket);
            inline_chunks.push((chunk_index, chunk));
            continue;
        };

        let worker_compare = Arc::clone(&compare);
        let worker_operation = operation.cloned();
        let worker_sender = sender.clone();
        let job = move |(chunk_index, mut chunk, permit): (usize, Vec<T>, BurstPermit)| {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                stage_sort_chunk(worker_operation.as_ref());
                check_operation(worker_operation.as_ref())?;
                chunk.sort_by(|left, right| worker_compare(left, right));
                check_operation(worker_operation.as_ref())
            }));
            match outcome {
                Ok(Ok(())) => {
                    let _ =
                        worker_sender.send(Ok((chunk_index, BurstEnvelope::new(permit, chunk))));
                }
                Ok(Err(error)) => {
                    drop(permit);
                    let _ = worker_sender.send(Err(error));
                }
                Err(panic) => {
                    drop(permit);
                    let _ = worker_sender.send(Err(SortError::WorkerPanicked));
                    resume_unwind(panic);
                }
            }
        };
        match executor.try_spawn_with_payload(ticket, (chunk_index, chunk, permit), job) {
            Ok(()) => spawned = spawned.saturating_add(1),
            Err((ticket, (chunk_index, chunk, permit), _job)) => {
                drop(ticket);
                drop(permit);
                inline_chunks.push((chunk_index, chunk));
            }
        }
    }
    drop(sender);

    for (_, chunk) in &mut inline_chunks {
        sort_inline_chunk(chunk, compare.as_ref(), operation)?;
    }
    let inline_count = inline_chunks.len();
    let mut sorted_chunks = (0..chunk_count).map(|_| None).collect::<Vec<_>>();
    for (chunk_index, chunk) in inline_chunks {
        let Some(slot) = sorted_chunks.get_mut(chunk_index) else {
            return Err(SortError::Invariant);
        };
        if slot.replace(chunk).is_some() {
            return Err(SortError::Invariant);
        }
    }
    let mut received = inline_count;
    while received < inline_count.saturating_add(spawned) {
        check_operation(operation)?;
        match receiver.recv_timeout(LEAF_WAIT_POLL) {
            Ok(Ok((chunk_index, envelope))) => {
                let Some(slot) = sorted_chunks.get_mut(chunk_index) else {
                    return Err(SortError::Invariant);
                };
                if slot.replace(envelope.into_value()).is_some() {
                    return Err(SortError::Invariant);
                }
                received = received.saturating_add(1);
            }
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err(SortError::WorkerDisconnected),
        }
    }
    check_operation(operation)?;
    let sorted_chunks = sorted_chunks
        .into_iter()
        .collect::<Option<Vec<Vec<T>>>>()
        .ok_or(SortError::Invariant)?;

    let (items, merge_pops) = match catch_unwind(AssertUnwindSafe(|| {
        merge_sorted_chunks(sorted_chunks, compare.as_ref(), operation)
    })) {
        Ok(result) => result?,
        Err(_) => return Err(SortError::WorkerPanicked),
    };
    #[cfg(not(test))]
    let _ = merge_pops;
    Ok(SortOutput {
        items,
        #[cfg(test)]
        metrics: SortMetrics {
            chunks: chunk_count,
            inline_chunks: inline_count,
            spawned_chunks: spawned,
            merge_pops,
        },
    })
}

fn sort_inline_chunk<T, F>(
    chunk: &mut [T],
    compare: &F,
    operation: Option<&OperationCtx>,
) -> Result<(), SortError>
where
    F: Fn(&T, &T) -> Ordering,
{
    stage_sort_chunk(operation);
    check_operation(operation)?;
    catch_unwind(AssertUnwindSafe(|| chunk.sort_by(compare)))
        .map_err(|_| SortError::WorkerPanicked)?;
    check_operation(operation)
}

fn merge_sorted_chunks<T, F>(
    chunks: Vec<Vec<T>>,
    compare: &F,
    operation: Option<&OperationCtx>,
) -> Result<(Vec<T>, usize), SortError>
where
    F: Fn(&T, &T) -> Ordering,
{
    if chunks.len() == 1 {
        return chunks
            .into_iter()
            .next()
            .map(|chunk| (chunk, 0))
            .ok_or(SortError::Invariant);
    }
    let total = chunks
        .iter()
        .try_fold(0_usize, |total, chunk| total.checked_add(chunk.len()))
        .ok_or(SortError::Invariant)?;
    stage_sort_merge(operation);
    check_operation(operation)?;
    let mut streams = chunks.into_iter().map(VecDeque::from).collect::<Vec<_>>();
    let mut heap = Vec::with_capacity(streams.len());
    for index in 0..streams.len() {
        if !streams[index].is_empty() {
            heap_push(&mut heap, index, &streams, compare)?;
        }
    }

    let mut output = Vec::with_capacity(total);
    while !heap.is_empty() {
        if !output.is_empty() && output.len() % MERGE_CANCEL_ITEMS == 0 {
            stage_sort_merge(operation);
            check_operation(operation)?;
        }
        let stream_index = heap_pop(&mut heap, &streams, compare)?;
        let item = streams
            .get_mut(stream_index)
            .and_then(VecDeque::pop_front)
            .ok_or(SortError::Invariant)?;
        output.push(item);
        if !streams[stream_index].is_empty() {
            heap_push(&mut heap, stream_index, &streams, compare)?;
        }
    }
    check_operation(operation)?;
    Ok((output, total))
}

fn heap_push<T, F>(
    heap: &mut Vec<usize>,
    value: usize,
    streams: &[VecDeque<T>],
    compare: &F,
) -> Result<(), SortError>
where
    F: Fn(&T, &T) -> Ordering,
{
    heap.push(value);
    let mut child = heap.len() - 1;
    while child > 0 {
        let parent = (child - 1) / 2;
        if !stream_precedes(heap[child], heap[parent], streams, compare)? {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
    Ok(())
}

fn heap_pop<T, F>(
    heap: &mut Vec<usize>,
    streams: &[VecDeque<T>],
    compare: &F,
) -> Result<usize, SortError>
where
    F: Fn(&T, &T) -> Ordering,
{
    if heap.is_empty() {
        return Err(SortError::Invariant);
    }
    let root = heap.swap_remove(0);
    let mut parent = 0;
    while parent < heap.len() {
        let left = parent.saturating_mul(2).saturating_add(1);
        if left >= heap.len() {
            break;
        }
        let right = left + 1;
        let child =
            if right < heap.len() && stream_precedes(heap[right], heap[left], streams, compare)? {
                right
            } else {
                left
            };
        if !stream_precedes(heap[child], heap[parent], streams, compare)? {
            break;
        }
        heap.swap(parent, child);
        parent = child;
    }
    Ok(root)
}

fn stream_precedes<T, F>(
    left: usize,
    right: usize,
    streams: &[VecDeque<T>],
    compare: &F,
) -> Result<bool, SortError>
where
    F: Fn(&T, &T) -> Ordering,
{
    let left_item = streams
        .get(left)
        .and_then(VecDeque::front)
        .ok_or(SortError::Invariant)?;
    let right_item = streams
        .get(right)
        .and_then(VecDeque::front)
        .ok_or(SortError::Invariant)?;
    let ordering = compare(left_item, right_item);
    Ok(ordering == Ordering::Less || (ordering == Ordering::Equal && left < right))
}

fn check_operation(operation: Option<&OperationCtx>) -> Result<(), SortError> {
    if operation.is_some_and(|operation| operation.check().is_err()) {
        Err(SortError::Cancelled)
    } else {
        Ok(())
    }
}

fn stage_sort_chunk(operation: Option<&OperationCtx>) {
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::SortChunk);
    }
    #[cfg(not(test))]
    let _ = operation;
}

fn stage_sort_merge(operation: Option<&OperationCtx>) {
    #[cfg(test)]
    if let Some(operation) = operation {
        operation.stage(TestStage::SortMerge);
    }
    #[cfg(not(test))]
    let _ = operation;
}

#[cfg(test)]
mod tests {
    use super::{SORT_CHUNK_ITEMS, SortError, sort_cancelable};
    use crate::file_executor::GrepGlobExecutor;
    use crate::operation::{RequestWorkGuard, TestStage};
    use rmcp::model::RequestId;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn p1_and_p4_match_the_independent_stable_sort_oracle() {
        let input = (0..SORT_CHUNK_ITEMS * 3 + 91)
            .map(|index| ((index * 7_919) % 997, index))
            .collect::<Vec<_>>();
        let mut oracle = input.clone();
        oracle.sort_by_key(|item| item.0);

        for parallelism in [1, 4] {
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism));
            let output = sort_cancelable(
                input.clone(),
                |left, right| left.0.cmp(&right.0),
                None,
                Some(&executor),
            )
            .unwrap();
            assert_eq!(output.items, oracle);
            assert_eq!(output.metrics.chunks, 4);
            if parallelism == 1 {
                assert_eq!(output.metrics.spawned_chunks, 0);
                assert_eq!(output.metrics.inline_chunks, 4);
            } else {
                assert!(output.metrics.spawned_chunks <= 3);
                assert_eq!(
                    output.metrics.inline_chunks + output.metrics.spawned_chunks,
                    4
                );
            }
            assert_eq!(output.metrics.merge_pops, oracle.len());
            executor.wait_for_test_quiescence();
            let burst = executor.test_burst_ledger();
            let tickets = executor.test_ticket_ledger();
            assert_eq!(burst.allocated, burst.released);
            assert_eq!(burst.live, 0);
            assert_eq!(burst.duplicate_releases, 0);
            assert_eq!(tickets.allocated, tickets.released);
            assert_eq!(tickets.live, 0);
            assert_eq!(tickets.duplicate_releases, 0);
        }
    }

    #[test]
    fn sort_chunk_and_merge_barriers_cancel_without_a_partial_order() {
        for target in [TestStage::SortChunk, TestStage::SortMerge] {
            let parent = CancellationToken::new();
            let cancel = parent.clone();
            let fired = Arc::new(AtomicBool::new(false));
            let fired_hook = Arc::clone(&fired);
            let (mut guard, operation) = RequestWorkGuard::new_with_hook(
                RequestId::String(Arc::from(format!("sort-{target:?}"))),
                parent,
                Arc::new(move |stage| {
                    if stage == target && !fired_hook.swap(true, Ordering::AcqRel) {
                        cancel.cancel();
                    }
                }),
            );
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(4));
            let input = (0..SORT_CHUNK_ITEMS * 3).rev().collect::<Vec<_>>();
            let error = sort_cancelable(input, usize::cmp, Some(&operation), Some(&executor))
                .err()
                .expect("the requested sort stage must observe cancellation");
            assert_eq!(error, SortError::Cancelled);
            assert!(fired.load(Ordering::Acquire));
            guard.disarm();
            executor.wait_for_test_quiescence();
            assert_eq!(executor.test_burst_available(), executor.extra_capacity());
            assert_eq!(executor.test_ticket_available(), executor.extra_capacity());
        }
    }

    #[test]
    fn comparator_panic_during_merge_is_an_explicit_sort_failure() {
        let merge_started = Arc::new(AtomicBool::new(false));
        let merge_hook = Arc::clone(&merge_started);
        let (mut guard, operation) = RequestWorkGuard::new_with_hook(
            RequestId::String(Arc::from("sort-merge-panic")),
            CancellationToken::new(),
            Arc::new(move |stage| {
                if stage == TestStage::SortMerge {
                    merge_hook.store(true, Ordering::Release);
                }
            }),
        );
        let compare_flag = Arc::clone(&merge_started);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(1));
        let error = sort_cancelable(
            (0..SORT_CHUNK_ITEMS * 2).rev().collect(),
            move |left: &usize, right: &usize| {
                assert!(
                    !compare_flag.load(Ordering::Acquire),
                    "injected comparator panic during merge"
                );
                left.cmp(right)
            },
            Some(&operation),
            Some(&executor),
        )
        .err()
        .expect("the merge panic must not escape the sort API");
        assert_eq!(error, SortError::WorkerPanicked);
        assert!(merge_started.load(Ordering::Acquire));
        guard.disarm();
        executor.wait_for_test_quiescence();
    }
}
