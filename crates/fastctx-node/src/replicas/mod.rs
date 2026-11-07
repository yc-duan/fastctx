//! Current plan and history copies on every eligible computer.

pub(crate) mod backend;
pub(crate) mod bodies;
pub(crate) mod files;
mod legacy;
pub(crate) mod location;
mod observations;
pub(crate) mod publication;
mod removal;
pub(crate) mod state;
pub(crate) mod sync;
pub(crate) mod updates;
pub(crate) mod writing;

#[derive(Default)]
pub(crate) struct Copies {
    pub state: state::State,
    pub updates: updates::Updates,
    pub repairs: Repairs,
}

/// Restorings of every projection, asked for at once, run once for them
/// all: each job that ends asks for one.
#[derive(Default)]
pub(crate) struct Repairs {
    asked: std::sync::atomic::AtomicU64,
    /// How many asks the restorings run so far began after.
    done: tokio::sync::Mutex<u64>,
}

impl Repairs {
    /// Runs `work`, unless a run that began after this ask ended meanwhile.
    pub(crate) async fn run<F>(&self, work: impl FnOnce() -> F) -> Result<(), String>
    where
        F: std::future::Future<Output = Result<(), String>>,
    {
        use std::sync::atomic::Ordering;
        let ask = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
        let mut done = self.done.lock().await;
        if *done >= ask {
            return Ok(());
        }
        let covers = self.asked.load(Ordering::Acquire);
        let result = work().await;
        *done = covers;
        result
    }
}
