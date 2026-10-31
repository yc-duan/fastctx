//! Parks synchronous callers on wakeups, without a runtime or polling sleeps.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

struct Signal {
    ready: Mutex<bool>,
    changed: Condvar,
}
impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.ready.lock().expect("wake state") = true;
        self.changed.notify_one();
    }
}

pub(super) fn wait<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    let signal = Arc::new(Signal { ready: Mutex::new(false), changed: Condvar::new() });
    let waker = Waker::from(signal.clone());
    let mut context = Context::from_waker(&waker);
    // This synchronous driver cannot yield to an enclosing Tokio task when
    // its cooperative budget expires; admission must wake for real readiness.
    let mut future = std::pin::pin!(tokio::task::unconstrained(future));
    let deadline = Instant::now() + timeout;
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        let mut ready = signal.ready.lock().expect("wake state");
        while !*ready {
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (next, timed) = signal.changed.wait_timeout(ready, remaining).expect("wake state");
            ready = next;
            if timed.timed_out() && !*ready {
                return None;
            }
        }
        *ready = false;
    }
}
