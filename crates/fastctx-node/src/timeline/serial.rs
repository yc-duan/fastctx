//! One build of a conversation's timeline at a time, held from the reading
//! of its history to the last write: a build its job gave up on, still
//! running, ends before the next one reads (BB05), so an older reading never
//! writes over a newer one.

use fastctx_types::ResidentId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// The builds of one kind under way, by conversation.
pub(super) struct Serial(LazyLock<Mutex<HashMap<ResidentId, Arc<tokio::sync::Mutex<()>>>>>);

impl Serial {
    pub(super) const fn new() -> Self {
        Self(LazyLock::new(Mutex::default))
    }

    /// Held while `resident`'s timeline is built: waits for the one before.
    /// Each piece of blocking work of the build holds it too (`Held`), so a
    /// build whose job gave up on it holds it until that work ends (CC02).
    pub(super) async fn hold(&self, resident: &ResidentId) -> Held {
        let turn = self.0.lock().expect("timeline builds").entry(resident.clone()).or_default().clone();
        Arc::new(turn.lock_owned().await)
    }
}

/// A build held: cloned into each piece of its blocking work.
pub(super) type Held = Arc<tokio::sync::OwnedMutexGuard<()>>;
