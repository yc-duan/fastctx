//! A source owns its transfer through checking and publication.

use std::sync::Arc;

/// The tries bringing one package in at once (`obtain`): only one of them
/// puts its package in place.
#[derive(Default)]
pub(super) struct Race {
    pub(super) won: Arc<tokio::sync::Mutex<bool>>,
    pub(super) finished: tokio::sync::Notify,
}

/// One try of bringing a package in: its own file, and where it stands
/// (`obtain`).
pub(super) struct Attempt {
    /// Numbers the try's file (`fastctx_update::Cache::attempt`).
    pub(super) id: u64,
    /// Where it stands: every change is one step of this, so a transfer
    /// cannot start once the try is given up, a package checked after it was
    /// given up is not put in place, and one being put in place finishes.
    state: tokio::sync::watch::Sender<Try>,
    /// The bytes it brought so far, which say whether it moves.
    pub(super) brought: std::sync::atomic::AtomicU64,
    pub(super) downloaded: std::sync::Mutex<fastctx_types::update::PackageProgress>,
    pub(super) race: Arc<Race>,
}

/// Where a try stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Try {
    /// Asked of a machine; nothing of it came yet.
    Asked,
    /// Its transfer runs.
    Running,
    /// Given up while its transfer runs: the transfer stops.
    Stopping,
    /// It has the whole package and checks it.
    Keeping,
    /// It checked its package and puts it in place, which it finishes.
    Committing,
    /// Publication was revoked, but checking and cleanup still have an owner.
    Revoked,
    /// Over: given up, failed, or kept.
    Over,
}

impl Attempt {
    pub(super) fn new(id: u64, race: Arc<Race>) -> Self {
        Self {
            id,
            state: tokio::sync::watch::channel(Try::Asked).0,
            brought: std::sync::atomic::AtomicU64::new(0),
            downloaded: std::sync::Mutex::new(Default::default()),
            race,
        }
    }

    /// Starts its transfer, unless it is given up or one runs already:
    /// whether it started.
    pub(super) fn begin(&self) -> bool {
        self.step(Try::Asked, Try::Running)
    }

    /// Its transfer has the whole package: whether it may check it, not
    /// given up meanwhile.
    pub(super) fn keep(&self) -> bool {
        self.step(Try::Running, Try::Keeping)
    }

    /// Publication is serialized across sources, and only successful storage
    /// wins. A failed publisher releases this guard for the next complete source.
    pub(super) async fn commit(&self) -> Option<tokio::sync::OwnedMutexGuard<bool>> {
        let won = self.race.won.clone().lock_owned().await;
        if *won || !self.step(Try::Keeping, Try::Committing) {
            return None;
        }
        Some(won)
    }

    pub(super) fn progress(&self, size: Option<u64>) -> fastctx_types::update::PackageProgress {
        let downloaded = *self.downloaded.lock().expect("never poisoned");
        fastctx_types::update::PackageProgress {
            received: downloaded.received.saturating_add(self.brought.load(std::sync::atomic::Ordering::Relaxed)),
            total: size.or(downloaded.total).map(|size| {
                size.saturating_add(if downloaded.received > 0 { downloaded.total.unwrap_or(0) } else { 0 })
            }),
        }
    }

    fn step(&self, from: Try, to: Try) -> bool {
        self.state.send_if_modified(|state| {
            let stepped = *state == from;
            if stepped {
                *state = to;
            }
            stepped
        })
    }

    /// Its transfer is over, whatever came of it.
    pub(super) fn over(&self) {
        self.state.send_replace(Try::Over);
    }

    /// Takes back the right of a try given up to put its package in place
    /// while it still checks it. This does not end its actual work; `ended`
    /// resolves only after that work and its cleanup finish.
    pub(super) fn revoke(&self) -> bool {
        self.state.send_if_modified(|state| {
            let revoked = *state == Try::Keeping;
            if revoked {
                *state = Try::Revoked;
            }
            revoked
        });
        *self.state.borrow() != Try::Committing
    }

    /// Gives the try up: one not begun is over at once, a transfer running
    /// is told to stop, and one with the whole package goes on to check it
    /// and put it in place, unless that right is taken back.
    pub(super) fn give_up(&self) {
        self.state.send_if_modified(|state| {
            let next = match *state {
                Try::Asked => Try::Over,
                Try::Running => Try::Stopping,
                other => other,
            };
            let changed = next != *state;
            *state = next;
            changed
        });
    }

    /// Resolves once its transfer is to stop.
    pub(super) async fn stopping(&self) {
        let mut state = self.state.subscribe();
        loop {
            let won = self.race.finished.notified();
            if *self.race.won.lock().await {
                return;
            }
            tokio::select! {
                _ = state.wait_for(|state| matches!(state, Try::Stopping | Try::Over)) => return,
                () = won => {}
            }
        }
    }

    /// Resolves once it is over.
    pub(super) async fn ended(&self) {
        let _ = self.state.subscribe().wait_for(|state| *state == Try::Over).await;
    }

    /// Whether it has the whole package, which it checks or puts in place.
    pub(super) fn keeping(&self) -> bool {
        matches!(*self.state.borrow(), Try::Keeping | Try::Committing | Try::Revoked)
    }

    pub(super) fn is_over(&self) -> bool {
        *self.state.borrow() == Try::Over
    }
}
