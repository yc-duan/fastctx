//! Flush copied files concurrently, then publish only after every flush succeeds.

use std::fs::File;
use std::io;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

pub(super) fn copied_files(
    count: usize,
    copy: impl FnOnce(&mpsc::SyncSender<File>) -> io::Result<()>,
) -> io::Result<()> {
    // At most one worker and one queued handle per available CPU, and never
    // more workers than files. Memory, handles and threads are independent of
    // the total archive size; the synchronous channel applies backpressure.
    let workers = count.min(thread::available_parallelism()?.get());
    let (sender, receiver) = mpsc::sync_channel::<File>(workers);
    let receiver = Arc::new(Mutex::new(receiver));
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let receiver = Arc::clone(&receiver);
            handles.push(scope.spawn(move || -> io::Result<()> {
                loop {
                    let file = receiver.lock().map_err(|_| io::Error::other("copy flush queue poisoned"))?.recv();
                    let Ok(file) = file else { return Ok(()) };
                    file.sync_all()?;
                }
            }));
        }
        drop(receiver);
        let mut result = copy(&sender);
        drop(sender);
        for handle in handles {
            let flushed =
                handle.join().map_err(|_| io::Error::other("copy flush worker panicked")).and_then(|value| value);
            if let Err(error) = flushed {
                result = Err(error);
            }
        }
        result
    })
}

#[cfg(test)]
#[test]
fn flush_failure_disconnects_producers() {
    let device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let error = copied_files(1, |sender| {
        for _ in 0..3 {
            sender.send(File::open(device)?).map_err(|_| io::Error::other("flush failed"))?;
        }
        Ok(())
    });
    assert!(error.is_err());
}
