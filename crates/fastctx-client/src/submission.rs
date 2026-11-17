//! Naming the calls a client makes (`fastctx_types::submission`).
//!
//! Every call a client makes carries a fresh name. A machine whose answer
//! is that the work is accepted is asked again under the same name, which
//! does the work no second time and says how it stands, until it says what
//! came of it.

use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use std::time::{Duration, Instant};

/// How long accepted work is asked after before its caller is told that it
/// is accepted and still being done.
pub(crate) const ASKS_FOR: Duration = Duration::from_secs(60 * 60);

/// A name no other call has, which stays for the outcome as long as the
/// machine answering it lets it.
pub(crate) fn fresh() -> std::io::Result<Submission> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(Submission { name: format!("{:032x}", u128::from_be_bytes(bytes)), waits_ms: u32::MAX })
}

/// Whether a call answered with `failure` since `began` is asked again:
/// its work is accepted, and it has not been asked after for too long.
pub(crate) fn asks_again(failure: &Failure, began: Instant) -> bool {
    failure.is_accepted() && began.elapsed() < ASKS_FOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_names_are_valid_and_differ() {
        let (one, other) = (fresh().unwrap(), fresh().unwrap());
        assert!(one.check().is_ok());
        assert_ne!(one.name, other.name);
    }

    #[test]
    fn only_accepted_work_is_asked_after() {
        let now = Instant::now();
        assert!(asks_again(&Failure::accepted("going"), now));
        assert!(!asks_again(&Failure::new("failed"), now));
        assert!(!asks_again(&Failure::not_run("busy"), now));
    }
}
