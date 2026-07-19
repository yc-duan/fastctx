//! Ordered parallel execution for independent per-item work.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

/// Applies `worker` to every item on a scoped thread pool and feeds results to
/// `consume` strictly in item order, exactly once per delivered item.
///
/// `consume` may return `ControlFlow::Break` to stop early; workers then stop
/// picking up new items and undelivered results are dropped. Ordering is
/// restored by buffering out-of-order results until their turn, so a worker
/// stalled on a low index lets the others run ahead and that reorder buffer
/// grows with the input rather than with the channel bound. With one thread
/// (or one item) everything runs inline on the caller's thread.
pub(crate) fn for_each_ordered<T, R>(
    items: &[T],
    threads: usize,
    worker: impl Fn(&T) -> R + Sync,
    mut consume: impl FnMut(usize, R) -> ControlFlow<()>,
) where
    T: Sync,
    R: Send,
{
    let threads = threads.min(items.len());
    if threads <= 1 {
        for (index, item) in items.iter().enumerate() {
            if consume(index, worker(item)).is_break() {
                return;
            }
        }
        return;
    }
    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::sync_channel::<(usize, R)>(threads.saturating_mul(4));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let sender = sender.clone();
            let next = &next;
            let cancelled = &cancelled;
            let worker = &worker;
            scope.spawn(move || {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= items.len() {
                        break;
                    }
                    if sender.send((index, worker(&items[index]))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut pending = BTreeMap::new();
        let mut expected = 0_usize;
        'deliver: while expected < items.len() {
            let Ok((index, result)) = receiver.recv() else {
                break;
            };
            pending.insert(index, result);
            while let Some(result) = pending.remove(&expected) {
                let index = expected;
                expected += 1;
                if consume(index, result).is_break() {
                    break 'deliver;
                }
            }
        }
        // Unblocks workers stuck on a full channel and stops new pickups so
        // the scope join cannot deadlock after an early break.
        cancelled.store(true, Ordering::Relaxed);
        drop(receiver);
    });
}

#[cfg(test)]
mod tests {
    use super::for_each_ordered;
    use std::ops::ControlFlow;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn results_arrive_in_item_order_despite_uneven_work() {
        let items: Vec<usize> = (0..200).collect();
        let mut delivered = Vec::new();
        for_each_ordered(
            &items,
            8,
            |item| {
                if item % 7 == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                *item * 2
            },
            |index, result| {
                delivered.push((index, result));
                ControlFlow::Continue(())
            },
        );
        let expected: Vec<(usize, usize)> = (0..200).map(|index| (index, index * 2)).collect();
        assert_eq!(delivered, expected);
    }

    #[test]
    fn early_break_stops_consumption_and_terminates() {
        let items: Vec<usize> = (0..500).collect();
        let scheduled = AtomicUsize::new(0);
        let mut consumed = Vec::new();
        for_each_ordered(
            &items,
            4,
            |item| {
                scheduled.fetch_add(1, Ordering::Relaxed);
                *item
            },
            |index, result| {
                assert_eq!(index, result);
                consumed.push(index);
                if index == 9 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        );
        assert_eq!(consumed, (0..10).collect::<Vec<_>>());
        // Every index is picked up at most once and never past the end, so an
        // early break can only lower this count. Do not tighten this into an
        // upper bound below the input length: when the worker holding the next
        // in-order index is descheduled, the other workers may legitimately
        // drain the whole input into the reorder buffer before the break is
        // reached. That is a scheduling outcome, not a contract, and asserting
        // it made this test flaky on loaded CI machines (2026-07-19).
        assert!(scheduled.load(Ordering::Relaxed) <= items.len());
    }

    #[test]
    fn single_thread_runs_inline_and_in_order() {
        let items = ["a", "b", "c"];
        let mut seen = Vec::new();
        for_each_ordered(
            &items,
            1,
            |item| item.to_uppercase(),
            |index, result| {
                seen.push((index, result));
                ControlFlow::Continue(())
            },
        );
        assert_eq!(
            seen,
            vec![
                (0, "A".to_string()),
                (1, "B".to_string()),
                (2, "C".to_string())
            ]
        );
    }
}
