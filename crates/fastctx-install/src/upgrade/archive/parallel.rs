//! Independent archive entries use at most one worker per available CPU.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

pub(super) fn entries<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> io::Result<R> + Sync) -> io::Result<Vec<R>> {
    let workers = items.len().min(thread::available_parallelism()?.get());
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let (next, work) = (&next, &work);
            handles.push(thread::Builder::new().spawn_scoped(scope, move || {
                let mut results = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { return Ok::<_, io::Error>(results) };
                    results.push((index, work(item)?));
                }
            })?);
        }
        let mut results = Vec::with_capacity(items.len());
        results.resize_with(items.len(), || None);
        let mut failure = None;
        for handle in handles {
            match handle.join().map_err(|_| io::Error::other("archive worker panicked")).and_then(|result| result) {
                Ok(values) => {
                    for (index, value) in values {
                        results[index] = Some(value);
                    }
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(results.into_iter().map(|value| value.expect("every archive entry was visited")).collect())
    })
}
