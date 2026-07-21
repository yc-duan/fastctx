//! Bounded, runner-based ordered execution for grep candidates.

use crate::file_executor::{BurstEnvelope, BurstPermit, BurstUse, GrepGlobExecutor};
use crate::operation::{EpochGuard, OpError, OperationCtx, WorkCtx, WorkStop};
use parking_lot::{Condvar, Mutex};
use std::any::Any;
#[cfg(test)]
use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const FRONTIER_WAIT_SLICE: Duration = Duration::from_millis(10);

/// Failures owned by the ordered execution state rather than by one candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderedError {
    Cancelled,
    GenerationOverflow,
}

impl From<OpError> for OrderedError {
    fn from(_: OpError) -> Self {
        Self::Cancelled
    }
}

/// Reducer-side control over the generation whose speculative results are valid.
pub(crate) struct OrderedReducer<R> {
    shared: Arc<RequestSpecShared<R>>,
}

impl<R> OrderedReducer<R> {
    /// Retires queued/running speculation and immediately drops every ready envelope.
    pub(crate) fn retire_generation(&self) -> Result<(), OrderedError> {
        self.shared.retire_generation(true)
    }
}

/// Runs independent work speculatively while delivering each reachable result in order.
pub(crate) fn for_each_ordered<T, R, W, P, C>(
    items: Arc<[T]>,
    operation: OperationCtx,
    executor: Arc<GrepGlobExecutor>,
    worker: W,
    panic_value: P,
    mut consume: C,
) -> Result<(), OrderedError>
where
    T: Send + Sync + 'static,
    R: Send + 'static,
    W: Fn(usize, &T, &WorkCtx) -> Result<R, WorkStop> + Send + Sync + 'static,
    P: Fn(usize, Box<dyn Any + Send>) -> R + Send + Sync + 'static,
    C: FnMut(usize, R, &OrderedReducer<R>) -> ControlFlow<()>,
{
    let shared = Arc::new(RequestSpecShared::new(executor.parallelism(), items.len()));
    let _retire_on_exit = RetireOnDrop {
        shared: Arc::clone(&shared),
    };
    let reducer = OrderedReducer {
        shared: Arc::clone(&shared),
    };
    let worker = Arc::new(worker);
    let panic_value = Arc::new(panic_value);

    while shared.next_reduce() < items.len() {
        operation.check()?;
        fill_epoch(
            &shared,
            &items,
            &operation,
            &executor,
            &worker,
            &panic_value,
        );

        let index = shared.next_reduce();
        let value = match shared.take_frontier(index, &operation)? {
            Frontier::Ready(envelope) => envelope.into_value(),
            Frontier::Inline => {
                let work = operation.inline_work();
                match catch_unwind(AssertUnwindSafe(|| worker(index, &items[index], &work))) {
                    Ok(Ok(value)) => value,
                    Ok(Err(WorkStop::RequestCancelled)) => return Err(OrderedError::Cancelled),
                    Ok(Err(WorkStop::EpochRetired)) => {
                        unreachable!("coordinator work never carries an epoch")
                    }
                    Err(payload) => panic_value(index, payload),
                }
            }
        };

        #[cfg(test)]
        operation.stage(crate::operation::TestStage::OrderedReduce);
        operation.check()?;
        if consume(index, value, &reducer).is_break() {
            return Ok(());
        }
        shared.advance(index);
    }
    Ok(())
}

fn fill_epoch<T, R, W, P>(
    shared: &Arc<RequestSpecShared<R>>,
    items: &Arc<[T]>,
    operation: &OperationCtx,
    executor: &Arc<GrepGlobExecutor>,
    worker: &Arc<W>,
    panic_value: &Arc<P>,
) where
    T: Send + Sync + 'static,
    R: Send + 'static,
    W: Fn(usize, &T, &WorkCtx) -> Result<R, WorkStop> + Send + Sync + 'static,
    P: Fn(usize, Box<dyn Any + Send>) -> R + Send + Sync + 'static,
{
    let Some((generation, vacancies)) = shared.fill_snapshot() else {
        return;
    };
    let maximum = vacancies.min(executor.extra_capacity());
    let tickets = executor.try_runner_tickets(maximum);
    for ticket in tickets {
        #[cfg(test)]
        operation.stage(crate::operation::TestStage::RunnerQueued);
        if operation.check().is_err() {
            drop(ticket);
            break;
        }
        let shared_for_job = Arc::clone(shared);
        let items_for_job = Arc::clone(items);
        let operation_for_job = operation.clone();
        let executor_for_job = Arc::clone(executor);
        let worker_for_job = Arc::clone(worker);
        let panic_value_for_job = Arc::clone(panic_value);
        let job = move || {
            #[cfg(test)]
            operation_for_job.stage(crate::operation::TestStage::RunnerStarted);
            let work =
                WorkCtx::speculative(operation_for_job, shared_for_job.epoch_guard(generation));
            if work.check().is_err() {
                return;
            }
            #[cfg(test)]
            work.stage(crate::operation::TestStage::BeforeBurstAcquire);
            if work.check().is_err() {
                return;
            }
            let Some(permit) = executor_for_job.try_burst(BurstUse::SearchSpeculation) else {
                return;
            };
            if work.check().is_err() {
                return;
            }
            #[cfg(test)]
            work.stage(crate::operation::TestStage::BeforeCandidateClaim);
            if work.check().is_err() {
                return;
            }
            let Some(lease) = shared_for_job.try_claim(generation, permit) else {
                return;
            };
            let index = lease.index();
            #[cfg(test)]
            work.stage(crate::operation::TestStage::AfterCandidateClaim);
            if work.check().is_err() {
                return;
            }

            let value = match catch_unwind(AssertUnwindSafe(|| {
                worker_for_job(index, &items_for_job[index], &work)
            })) {
                Ok(Ok(value)) => value,
                Ok(Err(WorkStop::RequestCancelled | WorkStop::EpochRetired)) => return,
                Err(payload) => panic_value_for_job(index, payload),
            };
            if work.check().is_err() {
                return;
            }
            #[cfg(test)]
            work.stage(crate::operation::TestStage::BeforeReadyPublish);
            if work.check().is_err() {
                return;
            }
            lease.publish(value);
        };
        if let Err((ticket, job)) = executor.try_spawn(ticket, job) {
            drop(ticket);
            drop(job);
            break;
        }
    }
}

struct RetireOnDrop<R> {
    shared: Arc<RequestSpecShared<R>>,
}

impl<R> Drop for RetireOnDrop<R> {
    fn drop(&mut self) {
        let _ = self.shared.retire_generation(false);
    }
}

enum Frontier<R> {
    Ready(BurstEnvelope<R>),
    Inline,
}

struct RequestSpecShared<R> {
    width: usize,
    candidate_count: usize,
    generation: Arc<AtomicU64>,
    next_runner_id: AtomicU64,
    state: Mutex<RequestSpecState<R>>,
    changed: Condvar,
    #[cfg(test)]
    lease_ledger: LeaseLedger,
    #[cfg(test)]
    publish_fault: AtomicUsize,
}

struct RequestSpecState<R> {
    generation: u64,
    next_reduce: usize,
    accepting: bool,
    live_slots: usize,
    #[cfg(test)]
    peak_slots: usize,
    slots: Box<[Option<SpecSlot<R>>]>,
}

enum SpecSlot<R> {
    RunningExtra {
        index: usize,
        generation: u64,
        runner_id: u64,
    },
    Ready {
        index: usize,
        generation: u64,
        envelope: BurstEnvelope<R>,
    },
}

impl<R> RequestSpecShared<R> {
    fn new(width: usize, candidate_count: usize) -> Self {
        let width = width.max(1);
        Self {
            width,
            candidate_count,
            generation: Arc::new(AtomicU64::new(0)),
            next_runner_id: AtomicU64::new(0),
            state: Mutex::new(RequestSpecState {
                generation: 0,
                next_reduce: 0,
                accepting: true,
                live_slots: 0,
                #[cfg(test)]
                peak_slots: 0,
                slots: (0..width)
                    .map(|_| None)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            lease_ledger: LeaseLedger::default(),
            #[cfg(test)]
            publish_fault: AtomicUsize::new(0),
        }
    }

    fn next_reduce(&self) -> usize {
        self.state.lock().next_reduce
    }

    fn epoch_guard(&self, expected: u64) -> EpochGuard {
        EpochGuard::new(expected, Arc::clone(&self.generation))
    }

    fn fill_snapshot(&self) -> Option<(u64, usize)> {
        let state = self.state.lock();
        if !state.accepting || state.next_reduce >= self.candidate_count {
            return None;
        }
        let end = state
            .next_reduce
            .saturating_add(self.width)
            .min(self.candidate_count);
        let vacancies = (state.next_reduce.saturating_add(1)..end)
            .filter(|index| Self::slot(&state, self.width, *index).is_none())
            .count();
        (vacancies > 0).then_some((state.generation, vacancies))
    }

    fn try_claim(
        self: &Arc<Self>,
        generation: u64,
        permit: BurstPermit,
    ) -> Option<RunningLease<R>> {
        let runner_id = self
            .next_runner_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .ok()?
            .checked_add(1)?;
        let mut lease = RunningLease::new(Arc::clone(self), generation, runner_id, permit);
        let mut state = self.state.lock();
        if !state.accepting || state.generation != generation {
            return None;
        }
        let end = state
            .next_reduce
            .saturating_add(self.width)
            .min(self.candidate_count);
        let index = (state.next_reduce.saturating_add(1)..end)
            .find(|index| Self::slot(&state, self.width, *index).is_none())?;
        lease.bind(index);
        *Self::slot_mut(&mut state, self.width, index) = Some(SpecSlot::RunningExtra {
            index,
            generation,
            runner_id,
        });
        state.live_slots += 1;
        #[cfg(test)]
        {
            state.peak_slots = state.peak_slots.max(state.live_slots);
        }
        debug_assert!(state.live_slots < self.width);
        drop(state);
        self.changed.notify_all();
        Some(lease)
    }

    fn take_frontier(
        &self,
        index: usize,
        operation: &OperationCtx,
    ) -> Result<Frontier<R>, OrderedError> {
        loop {
            operation.check()?;
            let mut state = self.state.lock();
            debug_assert_eq!(state.next_reduce, index);
            let current_generation = state.generation;
            let slot = Self::slot_mut(&mut state, self.width, index);
            match slot.as_ref() {
                Some(SpecSlot::Ready {
                    index: slot_index,
                    generation: slot_generation,
                    ..
                }) if *slot_index == index && *slot_generation == current_generation => {
                    let Some(SpecSlot::Ready { envelope, .. }) = slot.take() else {
                        unreachable!("the checked frontier slot remains ready")
                    };
                    state.live_slots -= 1;
                    return Ok(Frontier::Ready(envelope));
                }
                Some(SpecSlot::RunningExtra {
                    index: slot_index,
                    generation: slot_generation,
                    ..
                }) if *slot_index == index && *slot_generation == current_generation => {
                    self.changed.wait_for(&mut state, FRONTIER_WAIT_SLICE);
                }
                _ => return Ok(Frontier::Inline),
            }
        }
    }

    fn advance(&self, index: usize) {
        let mut state = self.state.lock();
        debug_assert_eq!(state.next_reduce, index);
        debug_assert!(Self::slot(&state, self.width, index).is_none());
        state.next_reduce = index.saturating_add(1);
        drop(state);
        self.changed.notify_all();
    }

    fn retire_generation(&self, reactivate: bool) -> Result<(), OrderedError> {
        let mut state = self.state.lock();
        state.accepting = false;
        for slot in &mut state.slots {
            *slot = None;
        }
        state.live_slots = 0;
        let Some(next) = state.generation.checked_add(1) else {
            drop(state);
            self.changed.notify_all();
            return Err(OrderedError::GenerationOverflow);
        };
        state.generation = next;
        self.generation.store(next, Ordering::Release);
        state.accepting = reactivate;
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    fn slot(state: &RequestSpecState<R>, width: usize, index: usize) -> Option<&SpecSlot<R>> {
        state.slots[index % width]
            .as_ref()
            .filter(|slot| match slot {
                SpecSlot::RunningExtra {
                    index: slot_index, ..
                }
                | SpecSlot::Ready {
                    index: slot_index, ..
                } => *slot_index == index,
            })
    }

    fn slot_mut(
        state: &mut RequestSpecState<R>,
        width: usize,
        index: usize,
    ) -> &mut Option<SpecSlot<R>> {
        &mut state.slots[index % width]
    }

    #[cfg(test)]
    fn inject_publish_fault(&self, fault: PublishFault) {
        self.publish_fault.store(fault as usize, Ordering::Release);
    }

    #[cfg(test)]
    fn maybe_fault(&self, fault: PublishFault) {
        if self
            .publish_fault
            .compare_exchange(
                fault as usize,
                PublishFault::None as usize,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            panic!("injected ordered-window publish fault: {fault:?}");
        }
    }
}

#[must_use = "a running lease must either publish or clear its exact running slot"]
struct RunningLease<R> {
    shared: Arc<RequestSpecShared<R>>,
    index: Option<usize>,
    generation: u64,
    runner_id: u64,
    permit: Option<BurstPermit>,
    armed: bool,
    #[cfg(test)]
    lease_id: u64,
}

impl<R> RunningLease<R> {
    fn new(
        shared: Arc<RequestSpecShared<R>>,
        generation: u64,
        runner_id: u64,
        permit: BurstPermit,
    ) -> Self {
        #[cfg(test)]
        let lease_id = shared.lease_ledger.allocate();
        Self {
            shared,
            index: None,
            generation,
            runner_id,
            permit: Some(permit),
            armed: true,
            #[cfg(test)]
            lease_id,
        }
    }

    fn bind(&mut self, index: usize) {
        debug_assert!(self.index.is_none());
        self.index = Some(index);
    }

    fn index(&self) -> usize {
        self.index
            .expect("only a claimed running lease reaches work")
    }

    fn publish(mut self, value: R) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        let mut envelope = Some(BurstEnvelope::new(permit, value));
        #[cfg(test)]
        self.shared.maybe_fault(PublishFault::AfterPermitTaken);
        let Some(index) = self.index else {
            return;
        };
        let mut state = self.shared.state.lock();
        let matches = state.accepting
            && state.generation == self.generation
            && matches!(
                RequestSpecShared::slot(&state, self.shared.width, index),
                Some(SpecSlot::RunningExtra {
                    index: slot_index,
                    generation,
                    runner_id,
                }) if *slot_index == index
                    && *generation == self.generation
                    && *runner_id == self.runner_id
            );
        if matches && let Some(envelope) = envelope.take() {
            *RequestSpecShared::slot_mut(&mut state, self.shared.width, index) =
                Some(SpecSlot::Ready {
                    index,
                    generation: self.generation,
                    envelope,
                });
            drop(state);
            self.shared.changed.notify_all();
            #[cfg(test)]
            self.shared.maybe_fault(PublishFault::AfterReadyStored);
            self.armed = false;
        }
    }
}

impl<R> Drop for RunningLease<R> {
    fn drop(&mut self) {
        if self.armed
            && let Some(index) = self.index
        {
            let mut state = self.shared.state.lock();
            let matches = matches!(
                RequestSpecShared::slot(&state, self.shared.width, index),
                Some(SpecSlot::RunningExtra {
                    index: slot_index,
                    generation,
                    runner_id,
                }) if *slot_index == index
                    && *generation == self.generation
                    && *runner_id == self.runner_id
            );
            if matches {
                *RequestSpecShared::slot_mut(&mut state, self.shared.width, index) = None;
                state.live_slots -= 1;
                drop(state);
                self.shared.changed.notify_all();
            }
        }
        #[cfg(test)]
        self.shared.lease_ledger.release(self.lease_id);
    }
}

#[cfg(test)]
#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublishFault {
    None = 0,
    AfterPermitTaken = 1,
    AfterReadyStored = 2,
}

#[cfg(test)]
#[derive(Default)]
struct LeaseLedger {
    state: Mutex<LeaseLedgerState>,
}

#[cfg(test)]
#[derive(Default)]
struct LeaseLedgerState {
    next_id: u64,
    allocated: usize,
    released: usize,
    live: BTreeSet<u64>,
    duplicate_releases: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LeaseLedgerSnapshot {
    allocated: usize,
    released: usize,
    live: usize,
    duplicate_releases: usize,
}

#[cfg(test)]
impl LeaseLedger {
    fn allocate(&self) -> u64 {
        let mut state = self.state.lock();
        state.next_id = state.next_id.checked_add(1).expect("lease id overflow");
        let id = state.next_id;
        state.allocated += 1;
        assert!(state.live.insert(id));
        id
    }

    fn release(&self, id: u64) {
        let mut state = self.state.lock();
        if state.live.remove(&id) {
            state.released += 1;
        } else {
            state.duplicate_releases += 1;
        }
    }

    fn snapshot(&self) -> LeaseLedgerSnapshot {
        let state = self.state.lock();
        LeaseLedgerSnapshot {
            allocated: state.allocated,
            released: state.released,
            live: state.live.len(),
            duplicate_releases: state.duplicate_releases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LeaseLedgerSnapshot, OrderedError, PublishFault, RequestSpecShared, for_each_ordered,
    };
    use crate::file_executor::{BurstUse, GrepGlobExecutor};
    use crate::operation::{RequestWorkGuard, TestStage};
    use rmcp::model::RequestId;
    use std::collections::BTreeSet;
    use std::ops::ControlFlow;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex, mpsc};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn operation(id: i64) -> (RequestWorkGuard, crate::operation::OperationCtx) {
        RequestWorkGuard::new(RequestId::Number(id), CancellationToken::new())
    }

    fn released(allocated: usize) -> LeaseLedgerSnapshot {
        LeaseLedgerSnapshot {
            allocated,
            released: allocated,
            live: 0,
            duplicate_releases: 0,
        }
    }

    fn assert_executor_ownership_released(executor: &GrepGlobExecutor) {
        for snapshot in [executor.test_burst_ledger(), executor.test_ticket_ledger()] {
            assert_eq!(snapshot.released, snapshot.allocated);
            assert_eq!(snapshot.live, 0);
            assert_eq!(snapshot.duplicate_releases, 0);
        }
        assert_eq!(executor.test_burst_available(), executor.extra_capacity());
        assert_eq!(executor.test_ticket_available(), executor.extra_capacity());
    }

    #[test]
    fn p1_runs_every_frontier_inline_and_in_order() {
        let (mut guard, operation) = operation(1);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(1));
        let items: Arc<[usize]> = (0..64).collect::<Vec<_>>().into();
        let mut delivered = Vec::new();
        for_each_ordered(
            items,
            operation,
            executor,
            |_, item, work| {
                work.check()?;
                Ok(*item * 2)
            },
            |_, _| unreachable!("inline arithmetic does not panic"),
            |index, value, _| {
                delivered.push((index, value));
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        guard.disarm();
        assert_eq!(
            delivered,
            (0..64).map(|index| (index, index * 2)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lease_moves_its_permit_to_ready_then_consume_releases_both_once() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let shared = Arc::new(RequestSpecShared::<usize>::new(2, 2));
        let permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
        let lease = shared.try_claim(0, permit).unwrap();
        assert_eq!(lease.index(), 1);
        lease.publish(41);
        assert_eq!(shared.lease_ledger.snapshot(), released(1));

        let (_guard, operation) = operation(2);
        assert!(matches!(
            shared.take_frontier(0, &operation).unwrap(),
            super::Frontier::Inline
        ));
        shared.advance(0);
        let super::Frontier::Ready(envelope) = shared.take_frontier(1, &operation).unwrap() else {
            panic!("published lease must become the next ready frontier")
        };
        assert_eq!(envelope.into_value(), 41);
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn abandoning_a_claimed_lease_clears_its_exact_slot_and_releases_once() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let shared = Arc::new(RequestSpecShared::<usize>::new(2, 2));
        let permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
        let lease = shared.try_claim(0, permit).unwrap();
        assert_eq!(lease.index(), 1);
        assert_eq!(shared.state.lock().live_slots, 1);

        drop(lease);

        let state = shared.state.lock();
        assert_eq!(state.live_slots, 0);
        assert!(RequestSpecShared::slot(&state, shared.width, 1).is_none());
        drop(state);
        assert_eq!(shared.lease_ledger.snapshot(), released(1));
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn publish_faults_never_orphan_a_running_slot_or_double_release_a_lease() {
        for fault in [
            PublishFault::AfterPermitTaken,
            PublishFault::AfterReadyStored,
        ] {
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
            let shared = Arc::new(RequestSpecShared::<usize>::new(2, 2));
            let permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
            let lease = shared.try_claim(0, permit).unwrap();
            shared.inject_publish_fault(fault);
            assert!(std::panic::catch_unwind(AssertUnwindSafe(|| lease.publish(7))).is_err());
            shared.retire_generation(false).unwrap();
            assert_eq!(shared.state.lock().live_slots, 0);
            assert_eq!(shared.lease_ledger.snapshot(), released(1));
            assert_executor_ownership_released(&executor);
        }
    }

    #[test]
    fn generation_retirement_rejects_old_publish_and_allows_the_new_generation() {
        let (_guard, operation) = operation(21);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let shared = Arc::new(RequestSpecShared::<usize>::new(2, 3));
        let old_generation = shared.fill_snapshot().unwrap().0;
        let old_work = crate::operation::WorkCtx::speculative(
            operation.clone(),
            shared.epoch_guard(old_generation),
        );
        let old_permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
        let old_lease = shared.try_claim(old_generation, old_permit).unwrap();

        shared.retire_generation(true).unwrap();
        assert_eq!(
            old_work.check(),
            Err(crate::operation::WorkStop::EpochRetired)
        );
        old_lease.publish(999);
        assert_eq!(shared.state.lock().live_slots, 0);

        let new_generation = shared.fill_snapshot().unwrap().0;
        assert_ne!(new_generation, old_generation);
        let new_permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
        let new_lease = shared.try_claim(new_generation, new_permit).unwrap();
        new_lease.publish(7);

        assert!(matches!(
            shared.take_frontier(0, &operation).unwrap(),
            super::Frontier::Inline
        ));
        shared.advance(0);
        let super::Frontier::Ready(envelope) = shared.take_frontier(1, &operation).unwrap() else {
            panic!("the replacement generation must publish normally")
        };
        assert_eq!(envelope.into_value(), 7);
        assert_eq!(shared.lease_ledger.snapshot(), released(2));
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn reducer_retirement_makes_the_exact_inline_retry_win_over_old_work() {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        enum Outcome {
            Overflow,
            Value(usize),
        }

        let (mut guard, operation) = operation(22);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let first_index_one = Arc::new(AtomicBool::new(true));
        let old_retired = Arc::new(AtomicBool::new(false));
        let (old_started_tx, old_started_rx) = mpsc::sync_channel(0);
        let (release_old_tx, release_old_rx) = mpsc::sync_channel(0);
        let release_old_rx = Arc::new(Mutex::new(release_old_rx));
        let first_index_one_by_worker = Arc::clone(&first_index_one);
        let old_retired_by_worker = Arc::clone(&old_retired);
        let release_old_rx_by_worker = Arc::clone(&release_old_rx);
        let mut delivered = Vec::new();

        for_each_ordered(
            Arc::<[usize]>::from(vec![0, 1, 2]),
            operation,
            Arc::clone(&executor),
            move |index, item, work| {
                if index == 0 {
                    return Ok(Outcome::Overflow);
                }
                if index == 1 && first_index_one_by_worker.swap(false, Ordering::AcqRel) {
                    old_started_tx.send(()).unwrap();
                    release_old_rx_by_worker
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .expect("retirement must release the old runner");
                    let stopped = work.check();
                    if stopped == Err(crate::operation::WorkStop::EpochRetired) {
                        old_retired_by_worker.store(true, Ordering::Release);
                    }
                    return stopped.map(|()| Outcome::Value(*item));
                }
                work.check()?;
                Ok(Outcome::Value(*item))
            },
            |index, _| Outcome::Value(index),
            |index, outcome, reducer| {
                if outcome == Outcome::Overflow {
                    old_started_rx
                        .recv_timeout(Duration::from_secs(5))
                        .expect("the old speculative runner must claim before retirement");
                    reducer.retire_generation().unwrap();
                    delivered.push((index, Outcome::Value(100)));
                    release_old_tx.send(()).unwrap();
                } else {
                    delivered.push((index, outcome));
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        guard.disarm();
        executor.wait_for_test_quiescence();
        assert_eq!(
            delivered,
            vec![
                (0, Outcome::Value(100)),
                (1, Outcome::Value(1)),
                (2, Outcome::Value(2)),
            ]
        );
        assert!(old_retired.load(Ordering::Acquire));
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn speculative_candidate_panic_is_published_in_order_without_an_orphan() {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        enum Outcome {
            Value(usize),
            Panicked(usize),
        }

        let (mut guard, operation) = operation(23);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let (panic_started_tx, panic_started_rx) = mpsc::sync_channel(0);
        let panic_started_rx = Arc::new(Mutex::new(panic_started_rx));
        let panic_started_rx_by_worker = Arc::clone(&panic_started_rx);
        let mut delivered = Vec::new();
        for_each_ordered(
            Arc::<[usize]>::from(vec![0, 1, 2]),
            operation,
            Arc::clone(&executor),
            move |index, item, work| {
                if index == 0 {
                    panic_started_rx_by_worker
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .expect("index 1 must run speculatively");
                } else if index == 1 {
                    panic_started_tx.send(()).unwrap();
                    panic!("injected candidate panic");
                }
                work.check()?;
                Ok(Outcome::Value(*item))
            },
            |index, _| Outcome::Panicked(index),
            |index, outcome, _| {
                delivered.push((index, outcome));
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        guard.disarm();
        executor.wait_for_test_quiescence();
        assert_eq!(
            delivered,
            vec![
                (0, Outcome::Value(0)),
                (1, Outcome::Panicked(1)),
                (2, Outcome::Value(2)),
            ]
        );
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn million_candidate_state_has_only_p_minus_one_live_heavy_slots() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(4));
        let shared = Arc::new(RequestSpecShared::<usize>::new(4, 1_000_000));
        let generation = shared.fill_snapshot().unwrap().0;
        let mut leases = Vec::new();
        for expected_index in 1..4 {
            let permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
            let lease = shared.try_claim(generation, permit).unwrap();
            assert_eq!(lease.index(), expected_index);
            leases.push(lease);
        }
        assert!(executor.try_burst(BurstUse::SearchSpeculation).is_none());
        {
            let state = shared.state.lock();
            assert_eq!(state.slots.len(), 4);
            assert_eq!(state.live_slots, 3);
            assert_eq!(state.peak_slots, 3);
        }
        drop(leases);
        assert_eq!(shared.state.lock().live_slots, 0);
        assert_eq!(shared.lease_ledger.snapshot(), released(3));
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn concurrent_claimers_get_unique_indices_and_never_exceed_the_window() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(8));
        let shared = Arc::new(RequestSpecShared::<usize>::new(8, 1_000_000));
        let generation = shared.fill_snapshot().unwrap().0;
        let start = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();
        for _ in 0..7 {
            let permit = executor.try_burst(BurstUse::SearchSpeculation).unwrap();
            let shared_for_thread = Arc::clone(&shared);
            let start_for_thread = Arc::clone(&start);
            threads.push(std::thread::spawn(move || {
                start_for_thread.wait();
                shared_for_thread.try_claim(generation, permit).unwrap()
            }));
        }
        start.wait();
        let leases = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let indices = leases
            .iter()
            .map(super::RunningLease::index)
            .collect::<BTreeSet<_>>();
        assert_eq!(indices, (1..8).collect());
        {
            let state = shared.state.lock();
            assert_eq!(state.slots.len(), 8);
            assert_eq!(state.live_slots, 7);
            assert_eq!(state.peak_slots, 7);
        }
        drop(leases);
        assert_eq!(shared.state.lock().live_slots, 0);
        assert_eq!(shared.lease_ledger.snapshot(), released(7));
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn delayed_runner_cancelled_before_burst_never_claims_a_candidate() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let executor_for_thread = Arc::clone(&executor);
        let (runner_started_tx, runner_started_rx) = mpsc::sync_channel(0);
        let (release_runner_tx, release_runner_rx) = mpsc::sync_channel(0);
        let release_runner_rx = Arc::new(Mutex::new(release_runner_rx));
        let release_runner_rx_by_hook = Arc::clone(&release_runner_rx);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::RunnerStarted {
                runner_started_tx.send(()).unwrap();
                release_runner_rx_by_hook
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the cancelled delayed runner must be released");
            }
        });
        let (guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(24), CancellationToken::new(), hook);
        let (inline_started_tx, inline_started_rx) = mpsc::sync_channel(0);
        let (release_inline_tx, release_inline_rx) = mpsc::sync_channel(0);
        let release_inline_rx = Arc::new(Mutex::new(release_inline_rx));
        let release_inline_rx_by_worker = Arc::clone(&release_inline_rx);
        let thread = std::thread::spawn(move || {
            for_each_ordered(
                Arc::<[usize]>::from(vec![0, 1, 2]),
                operation,
                executor_for_thread,
                move |index, item, work| {
                    if index == 0 {
                        inline_started_tx.send(()).unwrap();
                        release_inline_rx_by_worker
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .expect("the cancelled inline frontier must be released");
                    }
                    work.check()?;
                    Ok(*item)
                },
                |index, _| index,
                |_, _, _| ControlFlow::Continue(()),
            )
        });

        runner_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the private-pool runner must actually start");
        inline_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the coordinator base lane must actually start");
        drop(guard);
        release_runner_tx.send(()).unwrap();
        release_inline_tx.send(()).unwrap();
        assert_eq!(thread.join().unwrap(), Err(OrderedError::Cancelled));
        executor.wait_for_test_quiescence();
        assert_eq!(executor.test_burst_ledger().allocated, 0);
        let ticket_ledger = executor.test_ticket_ledger();
        assert_eq!(ticket_ledger.allocated, 1);
        assert_eq!(ticket_ledger.released, 1);
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn cancellation_at_every_runner_boundary_stops_before_later_work_or_publication() {
        for (request_id, cancelled_stage) in [
            (30, TestStage::RunnerQueued),
            (31, TestStage::RunnerStarted),
            (32, TestStage::BeforeBurstAcquire),
            (33, TestStage::BeforeCandidateClaim),
            (34, TestStage::AfterCandidateClaim),
            (35, TestStage::BeforeReadyPublish),
        ] {
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
            let cancellation = CancellationToken::new();
            let cancellation_for_hook = cancellation.clone();
            let stage_hits = Arc::new(AtomicUsize::new(0));
            let stage_hits_for_hook = Arc::clone(&stage_hits);
            let (stage_reached_tx, stage_reached_rx) = mpsc::channel();
            let hook = Arc::new(move |stage| {
                if stage == cancelled_stage
                    && stage_hits_for_hook.fetch_add(1, Ordering::AcqRel) == 0
                {
                    cancellation_for_hook.cancel();
                    stage_reached_tx.send(()).unwrap();
                }
            });
            let (guard, operation) =
                RequestWorkGuard::new_with_hook(RequestId::Number(request_id), cancellation, hook);
            let stage_reached_rx = Arc::new(Mutex::new(stage_reached_rx));
            let stage_reached_for_worker = Arc::clone(&stage_reached_rx);
            let worker_calls = Arc::new(AtomicUsize::new(0));
            let worker_calls_for_worker = Arc::clone(&worker_calls);
            let consume_calls = Arc::new(AtomicUsize::new(0));
            let consume_calls_for_consumer = Arc::clone(&consume_calls);

            let result = for_each_ordered(
                Arc::<[usize]>::from(vec![0, 1, 2]),
                operation,
                Arc::clone(&executor),
                move |index, item, work| {
                    if index == 0 {
                        stage_reached_for_worker
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .expect("the targeted runner stage must be reached");
                    } else {
                        worker_calls_for_worker.fetch_add(1, Ordering::AcqRel);
                    }
                    work.check()?;
                    Ok(*item)
                },
                |index, _| index,
                move |_, _, _| {
                    consume_calls_for_consumer.fetch_add(1, Ordering::AcqRel);
                    ControlFlow::Continue(())
                },
            );
            drop(guard);
            assert_eq!(result, Err(OrderedError::Cancelled), "{cancelled_stage:?}");
            assert_eq!(stage_hits.load(Ordering::Acquire), 1, "{cancelled_stage:?}");
            assert_eq!(
                consume_calls.load(Ordering::Acquire),
                0,
                "{cancelled_stage:?}"
            );
            let expected_worker_calls =
                usize::from(cancelled_stage == TestStage::BeforeReadyPublish);
            assert_eq!(
                worker_calls.load(Ordering::Acquire),
                expected_worker_calls,
                "{cancelled_stage:?}"
            );
            executor.wait_for_test_quiescence();
            assert_executor_ownership_released(&executor);
        }
    }

    #[test]
    fn cancellation_at_ordered_reduce_never_calls_the_consumer() {
        let cancellation = CancellationToken::new();
        let cancellation_for_hook = cancellation.clone();
        let reduce_hits = Arc::new(AtomicUsize::new(0));
        let reduce_hits_for_hook = Arc::clone(&reduce_hits);
        let hook = Arc::new(move |stage| {
            if stage == TestStage::OrderedReduce
                && reduce_hits_for_hook.fetch_add(1, Ordering::AcqRel) == 0
            {
                cancellation_for_hook.cancel();
            }
        });
        let (guard, operation) =
            RequestWorkGuard::new_with_hook(RequestId::Number(36), cancellation, hook);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(1));
        let consume_calls = Arc::new(AtomicUsize::new(0));
        let consume_calls_for_consumer = Arc::clone(&consume_calls);

        let result = for_each_ordered(
            Arc::<[usize]>::from(vec![0]),
            operation,
            Arc::clone(&executor),
            |_, item, work| {
                work.check()?;
                Ok(*item)
            },
            |index, _| index,
            move |_, _, _| {
                consume_calls_for_consumer.fetch_add(1, Ordering::AcqRel);
                ControlFlow::Continue(())
            },
        );
        drop(guard);
        assert_eq!(result, Err(OrderedError::Cancelled));
        assert_eq!(reduce_hits.load(Ordering::Acquire), 1);
        assert_eq!(consume_calls.load(Ordering::Acquire), 0);
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn early_break_retires_speculation_and_keeps_the_window_bounded() {
        let (mut guard, operation) = operation(3);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(4));
        let items: Arc<[usize]> = (0..10_000).collect::<Vec<_>>().into();
        let examined = Arc::new(AtomicUsize::new(0));
        let examined_by_worker = Arc::clone(&examined);
        let mut delivered = Vec::new();
        for_each_ordered(
            items,
            operation,
            Arc::clone(&executor),
            move |_, item, work| {
                work.check()?;
                examined_by_worker.fetch_add(1, Ordering::AcqRel);
                Ok(*item)
            },
            |_, _| usize::MAX,
            |index, value, _| {
                assert_eq!(index, value);
                delivered.push(index);
                if index == 9 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .unwrap();
        guard.disarm();
        assert_eq!(delivered, (0..10).collect::<Vec<_>>());
        assert!(examined.load(Ordering::Acquire) < 10_000);
        executor.wait_for_test_quiescence();
        assert_executor_ownership_released(&executor);
    }

    #[test]
    fn dropping_the_request_guard_cancels_a_waiting_frontier() {
        let (guard, operation) = operation(4);
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(2));
        let executor_for_thread = Arc::clone(&executor);
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_by_worker = Arc::clone(&entered);
        let release_by_worker = Arc::clone(&release);
        let thread = std::thread::spawn(move || {
            let items: Arc<[usize]> = vec![0, 1, 2].into();
            for_each_ordered(
                items,
                operation,
                executor_for_thread,
                move |index, item, work| {
                    if index == 1 {
                        entered_by_worker.store(true, Ordering::Release);
                        while !release_by_worker.load(Ordering::Acquire) {
                            work.check()?;
                            std::thread::yield_now();
                        }
                    }
                    Ok(*item)
                },
                |_, _| usize::MAX,
                |_, _, _| ControlFlow::Continue(()),
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !entered.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline, "runner never started");
            std::thread::yield_now();
        }
        drop(guard);
        release.store(true, Ordering::Release);
        assert_eq!(thread.join().unwrap(), Err(OrderedError::Cancelled));
        executor.wait_for_test_quiescence();
        assert_executor_ownership_released(&executor);
    }
}
