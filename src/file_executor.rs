//! Process-wide, try-only extra CPU capacity for grep and glob.

use rayon::{ThreadPool, ThreadPoolBuilder};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

#[cfg(test)]
use std::collections::BTreeSet;
#[cfg(test)]
use std::sync::{Condvar, Mutex};

const MAX_FILE_PARALLELISM: usize = 16;

/// The bounded subsystem currently borrowing one extra CPU credit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BurstUse {
    SearchSpeculation,
    TraversalExtra,
    SortExtra,
}

struct CreditState {
    capacity: usize,
    available: AtomicUsize,
    #[cfg(test)]
    ledger: OwnershipLedger,
}

impl CreditState {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            available: AtomicUsize::new(capacity),
            #[cfg(test)]
            ledger: OwnershipLedger::default(),
        }
    }

    fn try_acquire(&self) -> bool {
        let mut available = self.available.load(Ordering::Acquire);
        loop {
            if available == 0 {
                return false;
            }
            match self.available.compare_exchange_weak(
                available,
                available - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => available = observed,
            }
        }
    }

    fn release(&self) {
        let previous = self.available.fetch_add(1, Ordering::AcqRel);
        debug_assert!(previous < self.capacity, "credit released more than once");
    }

    fn available(&self) -> usize {
        self.available.load(Ordering::Acquire)
    }
}

/// Exactly `P-1` non-waiting credits shared by search, traversal, and sorting.
pub(crate) struct CpuBurstCredits {
    state: Arc<CreditState>,
}

impl CpuBurstCredits {
    fn new(capacity: usize) -> Self {
        Self {
            state: Arc::new(CreditState::new(capacity)),
        }
    }

    /// Tries once and returns immediately when every extra CPU lane is occupied.
    pub(crate) fn try_one(&self, use_: BurstUse) -> Option<BurstPermit> {
        if !self.state.try_acquire() {
            return None;
        }
        Some(BurstPermit {
            state: Arc::clone(&self.state),
            use_,
            #[cfg(test)]
            permit_id: self.state.ledger.allocate(),
        })
    }

    /// Acquires at most `maximum` currently free credits without registering a waiter.
    pub(crate) fn try_up_to(&self, maximum: usize, use_: BurstUse) -> Vec<BurstPermit> {
        let mut permits = Vec::with_capacity(maximum.min(self.state.capacity));
        for _ in 0..maximum {
            let Some(permit) = self.try_one(use_) else {
                break;
            };
            permits.push(permit);
        }
        permits
    }

    pub(crate) fn capacity(&self) -> usize {
        self.state.capacity
    }

    pub(crate) fn available(&self) -> usize {
        self.state.available()
    }
}

impl fmt::Debug for CpuBurstCredits {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuBurstCredits")
            .field("capacity", &self.capacity())
            .field("available", &self.available())
            .finish()
    }
}

/// The sole linear owner of one shared extra CPU credit.
#[must_use = "dropping the permit is the only way to return its extra CPU credit"]
pub(crate) struct BurstPermit {
    state: Arc<CreditState>,
    use_: BurstUse,
    #[cfg(test)]
    permit_id: u64,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the ordered-window and traversal batches consume this B5 foundation"
    )
)]
impl BurstPermit {
    pub(crate) const fn use_(&self) -> BurstUse {
        self.use_
    }
}

impl fmt::Debug for BurstPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("BurstPermit");
        debug.field("use", &self.use_);
        #[cfg(test)]
        debug.field("permit_id", &self.permit_id);
        debug.finish_non_exhaustive()
    }
}

impl Drop for BurstPermit {
    fn drop(&mut self) {
        #[cfg(test)]
        self.state.ledger.release(self.permit_id);
        self.state.release();
    }
}

/// A ready speculative value that keeps its extra CPU credit until ordered consumption.
pub(crate) struct BurstEnvelope<T> {
    _permit: BurstPermit,
    value: T,
}

impl<T> BurstEnvelope<T> {
    /// Moves a running lease's sole permit owner into a ready result.
    pub(crate) fn new(permit: BurstPermit, value: T) -> Self {
        Self {
            _permit: permit,
            value,
        }
    }

    /// Releases the ready credit and returns the value to the ordered reducer.
    pub(crate) fn into_value(self) -> T {
        let Self { _permit, value } = self;
        drop(_permit);
        value
    }
}

/// Exactly `P-1` non-waiting tickets for queued plus running private-pool leaves.
pub(crate) struct ExtraTaskTickets {
    state: Arc<CreditState>,
}

impl ExtraTaskTickets {
    fn new(capacity: usize) -> Self {
        Self {
            state: Arc::new(CreditState::new(capacity)),
        }
    }

    /// Tries once and returns immediately when the private leaf queue is full.
    pub(crate) fn try_one(&self) -> Option<RunnerTicket> {
        if !self.state.try_acquire() {
            return None;
        }
        Some(RunnerTicket {
            state: Arc::clone(&self.state),
            #[cfg(test)]
            ticket_id: self.state.ledger.allocate(),
        })
    }

    /// Acquires at most `maximum` currently free tickets without waiting.
    pub(crate) fn try_up_to(&self, maximum: usize) -> Vec<RunnerTicket> {
        let mut tickets = Vec::with_capacity(maximum.min(self.state.capacity));
        for _ in 0..maximum {
            let Some(ticket) = self.try_one() else {
                break;
            };
            tickets.push(ticket);
        }
        tickets
    }

    pub(crate) fn capacity(&self) -> usize {
        self.state.capacity
    }

    pub(crate) fn available(&self) -> usize {
        self.state.available()
    }
}

impl fmt::Debug for ExtraTaskTickets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtraTaskTickets")
            .field("capacity", &self.capacity())
            .field("available", &self.available())
            .finish()
    }
}

/// A linear queue slot that lives from successful submission until leaf exit.
#[must_use = "the ticket must move into a submitted leaf or be dropped by the coordinator"]
pub(crate) struct RunnerTicket {
    state: Arc<CreditState>,
    #[cfg(test)]
    ticket_id: u64,
}

impl fmt::Debug for RunnerTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("RunnerTicket");
        #[cfg(test)]
        debug.field("ticket_id", &self.ticket_id);
        debug.finish_non_exhaustive()
    }
}

impl Drop for RunnerTicket {
    fn drop(&mut self) {
        #[cfg(test)]
        self.state.ledger.release(self.ticket_id);
        self.state.release();
    }
}

/// A private, process-wide extra pool plus its two orthogonal try-only quotas.
pub(crate) struct GrepGlobExecutor {
    parallelism: usize,
    extra_threads: usize,
    extra_pool: OnceLock<Result<ThreadPool, Arc<str>>>,
    extra_disabled: AtomicBool,
    burst: Arc<CpuBurstCredits>,
    leaf_tasks: Arc<ExtraTaskTickets>,
    #[cfg(test)]
    probe: Arc<ExecutorProbe>,
}

impl GrepGlobExecutor {
    /// Returns the one process-wide executor shared by every production entry point.
    pub(crate) fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<GrepGlobExecutor>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| Arc::new(Self::new())))
    }

    /// Uses the machine's capped file parallelism without initializing a pool.
    pub(crate) fn new() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .clamp(1, MAX_FILE_PARALLELISM);
        Self::with_parallelism(parallelism)
    }

    fn with_parallelism(parallelism: usize) -> Self {
        let parallelism = parallelism.clamp(1, MAX_FILE_PARALLELISM);
        let extra_threads = parallelism - 1;
        Self {
            parallelism,
            extra_threads,
            extra_pool: OnceLock::new(),
            extra_disabled: AtomicBool::new(false),
            burst: Arc::new(CpuBurstCredits::new(extra_threads)),
            leaf_tasks: Arc::new(ExtraTaskTickets::new(extra_threads)),
            #[cfg(test)]
            probe: Arc::new(ExecutorProbe::default()),
        }
    }

    /// Returns `P`, including the coordinator's request-local base lane.
    pub(crate) const fn parallelism(&self) -> usize {
        self.parallelism
    }

    /// Returns `E=P-1`, the maximum number of process-wide extra lanes.
    pub(crate) const fn extra_capacity(&self) -> usize {
        self.extra_threads
    }

    /// Tries to borrow one extra CPU lane without waiting.
    pub(crate) fn try_burst(&self, use_: BurstUse) -> Option<BurstPermit> {
        self.extra_enabled()
            .then(|| self.burst.try_one(use_))
            .flatten()
    }

    /// Tries to borrow up to `maximum` extra CPU lanes without waiting.
    pub(crate) fn try_bursts(&self, maximum: usize, use_: BurstUse) -> Vec<BurstPermit> {
        if self.extra_enabled() {
            self.burst.try_up_to(maximum, use_)
        } else {
            Vec::new()
        }
    }

    /// Tries to reserve one queued/running leaf slot without waiting.
    pub(crate) fn try_runner_ticket(&self) -> Option<RunnerTicket> {
        self.extra_enabled()
            .then(|| self.leaf_tasks.try_one())
            .flatten()
    }

    /// Tries to reserve up to `maximum` queued/running leaf slots without waiting.
    pub(crate) fn try_runner_tickets(&self, maximum: usize) -> Vec<RunnerTicket> {
        if self.extra_enabled() {
            self.leaf_tasks.try_up_to(maximum)
        } else {
            Vec::new()
        }
    }

    /// Submits one already-ticketed FastCtx leaf, or returns it intact for inline work.
    pub(crate) fn try_spawn<J>(
        self: &Arc<Self>,
        ticket: RunnerTicket,
        job: J,
    ) -> Result<(), (RunnerTicket, J)>
    where
        J: FnOnce() + Send + 'static,
    {
        if !self.extra_enabled() {
            return Err((ticket, job));
        }
        let pool = match self.pool() {
            Ok(pool) => pool,
            Err(_) => return Err((ticket, job)),
        };
        if !self.extra_enabled() {
            return Err((ticket, job));
        }

        let executor = Arc::downgrade(self);
        #[cfg(test)]
        let probe = Arc::clone(&self.probe);
        pool.spawn(move || {
            let ticket = ticket;
            #[cfg(test)]
            probe.record_leaf_started();
            let outcome = catch_unwind(AssertUnwindSafe(job));
            if outcome.is_err()
                && let Some(executor) = executor.upgrade()
            {
                executor.disable_after_panic();
            }
            drop(ticket);
            #[cfg(test)]
            probe.record_leaf_finished();
        });
        Ok(())
    }

    /// Submits a ticketed leaf with recoverable payload, returning every owner
    /// intact when private-pool admission is unavailable.
    pub(crate) fn try_spawn_with_payload<P, J>(
        self: &Arc<Self>,
        ticket: RunnerTicket,
        payload: P,
        job: J,
    ) -> Result<(), (RunnerTicket, P, J)>
    where
        P: Send + 'static,
        J: FnOnce(P) + Send + 'static,
    {
        if !self.extra_enabled() {
            return Err((ticket, payload, job));
        }
        let pool = match self.pool() {
            Ok(pool) => pool,
            Err(_) => return Err((ticket, payload, job)),
        };
        if !self.extra_enabled() {
            return Err((ticket, payload, job));
        }

        let executor = Arc::downgrade(self);
        #[cfg(test)]
        let probe = Arc::clone(&self.probe);
        pool.spawn(move || {
            let ticket = ticket;
            #[cfg(test)]
            probe.record_leaf_started();
            let outcome = catch_unwind(AssertUnwindSafe(|| job(payload)));
            if outcome.is_err()
                && let Some(executor) = executor.upgrade()
            {
                executor.disable_after_panic();
            }
            drop(ticket);
            #[cfg(test)]
            probe.record_leaf_finished();
        });
        Ok(())
    }

    fn extra_enabled(&self) -> bool {
        self.extra_threads > 0 && !self.extra_disabled.load(Ordering::Acquire)
    }

    fn pool(self: &Arc<Self>) -> Result<&ThreadPool, Arc<str>> {
        if self.extra_threads == 0 {
            return Err(Arc::from("extra pool is disabled at P=1"));
        }
        let pool = self.extra_pool.get_or_init(|| self.build_pool());
        if pool.is_err() {
            self.extra_disabled.store(true, Ordering::Release);
        }
        pool.as_ref().map_err(Arc::clone)
    }

    fn build_pool(self: &Arc<Self>) -> Result<ThreadPool, Arc<str>> {
        #[cfg(test)]
        {
            self.probe.record_pool_build_attempt();
            if self.probe.force_build_failure.load(Ordering::Acquire) {
                return Err(Arc::from("injected private-pool build failure"));
            }
        }

        let weak_executor = Arc::downgrade(self);
        let builder = ThreadPoolBuilder::new()
            .num_threads(self.extra_threads)
            .thread_name(|index| format!("fastctx-extra-{index}"))
            .panic_handler(move |_| {
                if let Some(executor) = weak_executor.upgrade() {
                    executor.disable_after_panic();
                }
            });
        #[cfg(test)]
        let builder = {
            let start_probe = Arc::clone(&self.probe);
            let exit_probe = Arc::clone(&self.probe);
            builder
                .start_handler(move |_| start_probe.record_worker_started())
                .exit_handler(move |_| exit_probe.record_worker_exited())
        };
        builder
            .build()
            .map_err(|error| Arc::<str>::from(error.to_string()))
    }

    fn disable_after_panic(&self) {
        self.extra_disabled.store(true, Ordering::Release);
        #[cfg(test)]
        self.probe.record_leaf_panic();
    }

    #[cfg(test)]
    pub(crate) fn with_test_parallelism(parallelism: usize) -> Self {
        Self::with_parallelism(parallelism)
    }

    #[cfg(test)]
    pub(crate) fn test_burst_available(&self) -> usize {
        self.burst.available()
    }

    #[cfg(test)]
    pub(crate) fn test_ticket_available(&self) -> usize {
        self.leaf_tasks.available()
    }

    #[cfg(test)]
    pub(crate) fn test_burst_ledger(&self) -> LedgerSnapshot {
        self.burst.state.ledger.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn test_ticket_ledger(&self) -> LedgerSnapshot {
        self.leaf_tasks.state.ledger.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn wait_for_test_quiescence(&self) {
        self.probe.wait_for(|state| {
            state.leaves_finished == state.leaves_started
                && self.leaf_tasks.available() == self.extra_threads
                && self.burst.available() == self.extra_threads
        });
    }
}

impl Default for GrepGlobExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for GrepGlobExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GrepGlobExecutor")
            .field("parallelism", &self.parallelism)
            .field("extra_threads", &self.extra_threads)
            .field("pool_initialized", &self.extra_pool.get().is_some())
            .field(
                "extra_disabled",
                &self.extra_disabled.load(Ordering::Acquire),
            )
            .field("burst", &self.burst)
            .field("leaf_tasks", &self.leaf_tasks)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[derive(Default)]
struct OwnershipLedger {
    state: Mutex<OwnershipLedgerState>,
}

#[cfg(test)]
#[derive(Default)]
struct OwnershipLedgerState {
    next_id: u64,
    allocated: usize,
    released: usize,
    live: BTreeSet<u64>,
    duplicate_releases: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LedgerSnapshot {
    pub(crate) allocated: usize,
    pub(crate) released: usize,
    pub(crate) live: usize,
    pub(crate) duplicate_releases: usize,
}

#[cfg(test)]
impl OwnershipLedger {
    fn allocate(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.next_id = state
            .next_id
            .checked_add(1)
            .expect("test ledger id overflow");
        let id = state.next_id;
        state.allocated += 1;
        assert!(state.live.insert(id));
        id
    }

    fn release(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        if state.live.remove(&id) {
            state.released += 1;
        } else {
            state.duplicate_releases += 1;
        }
    }

    fn snapshot(&self) -> LedgerSnapshot {
        let state = self.state.lock().unwrap();
        LedgerSnapshot {
            allocated: state.allocated,
            released: state.released,
            live: state.live.len(),
            duplicate_releases: state.duplicate_releases,
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct ExecutorProbe {
    force_build_failure: AtomicBool,
    state: Mutex<ExecutorProbeState>,
    changed: Condvar,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct ExecutorProbeState {
    pool_build_attempts: usize,
    workers_started: usize,
    workers_exited: usize,
    leaves_started: usize,
    leaves_finished: usize,
    active_leaves: usize,
    peak_active_leaves: usize,
    leaf_panics: usize,
}

#[cfg(test)]
impl ExecutorProbe {
    fn update(&self, update: impl FnOnce(&mut ExecutorProbeState)) {
        let mut state = self.state.lock().unwrap();
        update(&mut state);
        self.changed.notify_all();
    }

    fn record_pool_build_attempt(&self) {
        self.update(|state| state.pool_build_attempts += 1);
    }

    fn record_worker_started(&self) {
        self.update(|state| state.workers_started += 1);
    }

    fn record_worker_exited(&self) {
        self.update(|state| state.workers_exited += 1);
    }

    fn record_leaf_started(&self) {
        self.update(|state| {
            state.leaves_started += 1;
            state.active_leaves += 1;
            state.peak_active_leaves = state.peak_active_leaves.max(state.active_leaves);
        });
    }

    fn record_leaf_finished(&self) {
        self.update(|state| {
            debug_assert!(state.active_leaves > 0);
            state.active_leaves -= 1;
            state.leaves_finished += 1;
        });
    }

    fn record_leaf_panic(&self) {
        self.update(|state| state.leaf_panics += 1);
    }

    fn snapshot(&self) -> ExecutorProbeState {
        *self.state.lock().unwrap()
    }

    fn wait_for(&self, predicate: impl Fn(ExecutorProbeState) -> bool) -> ExecutorProbeState {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut state = self.state.lock().unwrap();
        while !predicate(*state) {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("executor probe timed out");
            let (next, timeout) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
            assert!(!timeout.timed_out(), "executor probe timed out: {state:?}");
        }
        *state
    }
}

#[cfg(test)]
mod tests {
    use super::{BurstUse, GrepGlobExecutor, LedgerSnapshot};
    use std::collections::BTreeSet;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Condvar, Mutex, mpsc};
    use std::time::Duration;

    fn executor(parallelism: usize) -> Arc<GrepGlobExecutor> {
        Arc::new(GrepGlobExecutor::with_parallelism(parallelism))
    }

    fn run_inline_base_work(values: &[usize]) -> usize {
        values.iter().sum()
    }

    fn released(allocated: usize) -> LedgerSnapshot {
        LedgerSnapshot {
            allocated,
            released: allocated,
            live: 0,
            duplicate_releases: 0,
        }
    }

    struct OpenGateOnDrop {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl OpenGateOnDrop {
        fn new() -> Self {
            Self {
                gate: Arc::new((Mutex::new(false), Condvar::new())),
            }
        }

        fn clone_gate(&self) -> Arc<(Mutex<bool>, Condvar)> {
            Arc::clone(&self.gate)
        }
    }

    impl Drop for OpenGateOnDrop {
        fn drop(&mut self) {
            let (lock, changed) = &*self.gate;
            *lock.lock().unwrap() = true;
            changed.notify_all();
        }
    }

    fn wait_for_open_gate(gate: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, changed) = &**gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
    }

    #[test]
    fn p1_has_no_extra_capacity_and_never_initializes_rayon() {
        let executor = executor(1);
        assert_eq!(executor.parallelism(), 1);
        assert_eq!(executor.extra_capacity(), 0);
        assert!(executor.try_burst(BurstUse::SearchSpeculation).is_none());
        assert!(executor.try_runner_ticket().is_none());
        assert!(executor.try_bursts(10, BurstUse::TraversalExtra).is_empty());
        assert!(executor.try_runner_tickets(10).is_empty());
        assert!(executor.extra_pool.get().is_none());
        assert_eq!(executor.probe.snapshot().pool_build_attempts, 0);
    }

    #[test]
    fn credits_and_tickets_are_bounded_try_only_linear_owners() {
        let executor = executor(4);
        let mut permits = executor.try_bursts(99, BurstUse::SortExtra);
        let mut tickets = executor.try_runner_tickets(99);
        assert_eq!(permits.len(), 3);
        assert_eq!(tickets.len(), 3);
        assert_eq!(executor.burst.available(), 0);
        assert_eq!(executor.leaf_tasks.available(), 0);
        assert!(executor.try_burst(BurstUse::TraversalExtra).is_none());
        assert!(executor.try_runner_ticket().is_none());
        assert!(
            permits
                .iter()
                .all(|permit| permit.use_() == BurstUse::SortExtra)
        );

        drop(permits.pop());
        drop(tickets.pop());
        permits.push(executor.try_burst(BurstUse::TraversalExtra).unwrap());
        tickets.push(executor.try_runner_ticket().unwrap());
        assert_eq!(executor.burst.available(), 0);
        assert_eq!(executor.leaf_tasks.available(), 0);
        drop(permits);
        drop(tickets);

        assert_eq!(executor.burst.available(), 3);
        assert_eq!(executor.leaf_tasks.available(), 3);
        assert_eq!(executor.burst.state.ledger.snapshot(), released(4));
        assert_eq!(executor.leaf_tasks.state.ledger.snapshot(), released(4));
    }

    #[test]
    fn concurrent_credit_attempts_never_cross_the_shared_capacity() {
        let executor = executor(4);
        let contenders = 32;
        let acquired = Arc::new(Barrier::new(contenders + 1));
        let release = Arc::new(Barrier::new(contenders + 1));
        let holders = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..contenders {
            let executor = Arc::clone(&executor);
            let acquired = Arc::clone(&acquired);
            let release = Arc::clone(&release);
            let holders = Arc::clone(&holders);
            let peak = Arc::clone(&peak);
            threads.push(std::thread::spawn(move || {
                let permit = executor.try_burst(BurstUse::SearchSpeculation);
                if permit.is_some() {
                    let current = holders.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(current, Ordering::AcqRel);
                }
                acquired.wait();
                release.wait();
                if permit.is_some() {
                    holders.fetch_sub(1, Ordering::AcqRel);
                }
                drop(permit);
            }));
        }
        acquired.wait();
        assert_eq!(holders.load(Ordering::Acquire), 3);
        assert_eq!(peak.load(Ordering::Acquire), 3);
        release.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(executor.burst.available(), 3);
        assert_eq!(executor.burst.state.ledger.snapshot(), released(3));
    }

    #[test]
    fn build_failure_is_latched_and_returns_the_job_for_inline_progress() {
        let executor = executor(4);
        executor
            .probe
            .force_build_failure
            .store(true, Ordering::Release);
        let ran_inline = Arc::new(AtomicBool::new(false));
        let ran_inline_job = Arc::clone(&ran_inline);
        let ticket = executor.try_runner_ticket().unwrap();
        let (ticket, job) = executor
            .try_spawn(ticket, move || {
                ran_inline_job.store(true, Ordering::Release);
            })
            .expect_err("the injected pool build must return the leaf intact");
        assert!(!ran_inline.load(Ordering::Acquire));
        drop(ticket);
        job();
        assert!(ran_inline.load(Ordering::Acquire));
        assert!(executor.extra_disabled.load(Ordering::Acquire));
        assert!(executor.extra_pool.get().unwrap().is_err());
        assert_eq!(executor.probe.snapshot().pool_build_attempts, 1);
        assert!(executor.try_runner_ticket().is_none());
        assert!(executor.try_burst(BurstUse::TraversalExtra).is_none());
        assert_eq!(executor.leaf_tasks.state.ledger.snapshot(), released(1));
    }

    #[test]
    fn leaf_panic_returns_its_ticket_and_disables_later_extra_admission() {
        let executor = executor(3);
        let ticket = executor.try_runner_ticket().unwrap();
        assert!(
            executor
                .try_spawn(ticket, || panic!("injected leaf panic"))
                .is_ok(),
            "the private pool must accept the leaf"
        );
        let state = executor.probe.wait_for(|state| state.leaves_finished == 1);
        assert_eq!(state.leaf_panics, 1);
        assert!(executor.extra_disabled.load(Ordering::Acquire));
        assert!(executor.try_runner_ticket().is_none());
        assert!(executor.try_burst(BurstUse::SearchSpeculation).is_none());
        assert_eq!(executor.leaf_tasks.available(), 2);
        assert_eq!(executor.leaf_tasks.state.ledger.snapshot(), released(1));
    }

    #[test]
    fn saturated_private_pool_does_not_gate_inline_base_work() {
        let executor = executor(3);
        let permits = executor.try_bursts(2, BurstUse::SearchSpeculation);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started_tx, started_rx) = mpsc::channel();
        for ticket in executor.try_runner_tickets(2) {
            let gate = Arc::clone(&gate);
            let started_tx = started_tx.clone();
            assert!(
                executor
                    .try_spawn(ticket, move || {
                        started_tx.send(()).unwrap();
                        let (lock, changed) = &*gate;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                    })
                    .is_ok()
            );
        }
        drop(started_tx);
        for _ in 0..2 {
            started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        assert!(executor.try_burst(BurstUse::SortExtra).is_none());
        assert!(executor.try_runner_ticket().is_none());

        assert_eq!(run_inline_base_work(&[1, 2, 3, 4]), 10);
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_all();
        executor.probe.wait_for(|state| state.leaves_finished == 2);
        drop(permits);
        assert_eq!(executor.burst.state.ledger.snapshot(), released(2));
        assert_eq!(executor.leaf_tasks.state.ledger.snapshot(), released(2));
    }

    #[test]
    fn eight_request_base_lanes_share_constant_extras_and_later_small_progresses() {
        const REQUESTS: usize = 8;
        const PARALLELISM: usize = 4;
        const EXTRAS: usize = PARALLELISM - 1;

        let executor = executor(PARALLELISM);
        let release = OpenGateOnDrop::new();
        let base_active = Arc::new(AtomicUsize::new(0));
        let peak_base_active = Arc::new(AtomicUsize::new(0));
        let accepted_leaves = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = mpsc::channel();
        let mut requests = Vec::new();

        for request_id in 0..REQUESTS {
            let executor_for_request = Arc::clone(&executor);
            let gate_for_request = release.clone_gate();
            let base_active_for_request = Arc::clone(&base_active);
            let peak_base_for_request = Arc::clone(&peak_base_active);
            let accepted_for_request = Arc::clone(&accepted_leaves);
            let ready_for_request = ready_tx.clone();
            requests.push(std::thread::spawn(move || {
                let active = base_active_for_request.fetch_add(1, Ordering::AcqRel) + 1;
                peak_base_for_request.fetch_max(active, Ordering::AcqRel);

                if let Some(ticket) = executor_for_request.try_runner_ticket() {
                    if let Some(permit) =
                        executor_for_request.try_burst(BurstUse::SearchSpeculation)
                    {
                        let gate_for_leaf = Arc::clone(&gate_for_request);
                        let spawned = executor_for_request
                            .try_spawn(ticket, move || {
                                let _permit = permit;
                                wait_for_open_gate(&gate_for_leaf);
                            })
                            .is_ok();
                        assert!(
                            spawned,
                            "the shared private pool must accept a ticketed leaf"
                        );
                        accepted_for_request.fetch_add(1, Ordering::AcqRel);
                    } else {
                        drop(ticket);
                    }
                }

                ready_for_request.send(request_id).unwrap();
                wait_for_open_gate(&gate_for_request);
                base_active_for_request.fetch_sub(1, Ordering::AcqRel);
            }));
        }
        drop(ready_tx);
        let ready = (0..REQUESTS)
            .map(|_| ready_rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(ready, (0..REQUESTS).collect());
        let held = executor
            .probe
            .wait_for(|state| state.workers_started == EXTRAS && state.active_leaves == EXTRAS);
        let held_tickets = executor.test_ticket_ledger();
        let held_bursts = executor.test_burst_ledger();
        let held_base = base_active.load(Ordering::Acquire);
        let later_small = run_inline_base_work(&[1, 2, 3, 4]);

        drop(release);
        for request in requests {
            request.join().unwrap();
        }
        executor.wait_for_test_quiescence();

        assert_eq!(accepted_leaves.load(Ordering::Acquire), EXTRAS);
        assert_eq!(held.pool_build_attempts, 1);
        assert_eq!(held.workers_started, EXTRAS);
        assert_eq!(held.active_leaves, EXTRAS);
        assert_eq!(held.peak_active_leaves, EXTRAS);
        assert_eq!(held_tickets.live, EXTRAS);
        assert_eq!(held_bursts.live, EXTRAS);
        assert_eq!(held_base, REQUESTS);
        assert_eq!(peak_base_active.load(Ordering::Acquire), REQUESTS);
        assert!(held_base + held.active_leaves <= REQUESTS + EXTRAS);
        assert_eq!(later_small, 10);
        assert_eq!(base_active.load(Ordering::Acquire), 0);
        assert_eq!(executor.test_ticket_ledger(), released(EXTRAS));
        assert_eq!(executor.test_burst_ledger(), released(EXTRAS));
    }

    #[test]
    fn private_workers_exit_after_the_last_executor_owner_is_dropped() {
        let executor = executor(4);
        let probe = Arc::clone(&executor.probe);
        let ticket = executor.try_runner_ticket().unwrap();
        assert!(executor.try_spawn(ticket, || {}).is_ok());
        probe.wait_for(|state| state.workers_started == 3 && state.leaves_finished == 1);
        drop(executor);
        let state = probe.wait_for(|state| state.workers_exited == 3);
        assert_eq!(state.pool_build_attempts, 1);
        assert_eq!(state.workers_started, 3);
        assert_eq!(state.workers_exited, 3);
    }

    #[test]
    fn private_pool_size_ignores_rayon_num_threads() {
        const CHILD_MARKER: &str = "FASTCTX_TEST_RAYON_ENV_CHILD";
        if std::env::var_os(CHILD_MARKER).is_some() {
            assert_private_pool_has_three_overlapping_workers();
            return;
        }

        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .arg("--exact")
            .arg("file_executor::tests::private_pool_size_ignores_rayon_num_threads")
            .arg("--nocapture")
            .env(CHILD_MARKER, "1")
            .env("RAYON_NUM_THREADS", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = child.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "RAYON_NUM_THREADS child exceeded its hard deadline\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_private_pool_has_three_overlapping_workers() {
        let executor = executor(4);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started_tx, started_rx) = mpsc::channel();
        for ticket in executor.try_runner_tickets(3) {
            let gate = Arc::clone(&gate);
            let started_tx = started_tx.clone();
            assert!(
                executor
                    .try_spawn(ticket, move || {
                        started_tx
                            .send(rayon::current_thread_index().unwrap())
                            .unwrap();
                        let (lock, changed) = &*gate;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                    })
                    .is_ok()
            );
        }
        drop(started_tx);

        let mut indices = BTreeSet::new();
        let mut receive_failure = None;
        for _ in 0..3 {
            match started_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(index) => {
                    indices.insert(index);
                }
                Err(error) => {
                    receive_failure = Some(error);
                    break;
                }
            }
        }
        let (lock, changed) = &*gate;
        *lock.lock().unwrap() = true;
        changed.notify_all();
        executor.probe.wait_for(|state| state.leaves_finished == 3);
        assert!(
            receive_failure.is_none(),
            "private workers did not overlap: {receive_failure:?}"
        );
        assert_eq!(indices, BTreeSet::from([0, 1, 2]));
        assert_eq!(executor.probe.snapshot().workers_started, 3);
    }
}
