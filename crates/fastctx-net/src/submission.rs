//! The submission a task answers (`fastctx_types::submission`), carried
//! beside the work it does.
//!
//! Whoever takes a named call in runs its answering inside `scope`. What
//! answers it reads the name with `current` where it submits work, and a
//! call it makes of another machine on the way carries the same name, so
//! asking the whole thing again asks each part of it again under the name
//! it had.

use fastctx_types::submission::Submission;
use std::future::Future;

tokio::task_local! {
    static ANSWERED: Option<Submission>;
}

/// The submission the running task answers, if its call was named.
pub fn current() -> Option<Submission> {
    ANSWERED.try_with(Clone::clone).ok().flatten()
}

/// Runs `answering` as the answer to `submission`.
pub async fn scope<F: Future>(submission: Option<Submission>, answering: F) -> F::Output {
    ANSWERED.scope(submission, answering).await
}

/// The submission as the machine answering it knows it: its name bound to
/// who asked, so one asker's names are no other's. `asker` is every fact
/// that tells askers apart, such as the account and the machine.
pub fn bound(asker: &[&str], submission: &Submission) -> Submission {
    let mut named = Vec::new();
    for part in asker.iter().copied().chain([submission.name.as_str()]) {
        named.extend_from_slice(&(part.len() as u64).to_le_bytes());
        named.extend_from_slice(part.as_bytes());
    }
    let digest = crate::sign::sha256(&named);
    let name = digest[..16].iter().map(|byte| format!("{byte:02x}")).collect();
    Submission { name, waits_ms: submission.waits_ms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_bound_to_its_asker() {
        let asked = Submission { name: "n".into(), waits_ms: 7 };
        let here = bound(&["account", "machine"], &asked);
        assert_eq!(here, bound(&["account", "machine"], &asked));
        assert_eq!(here.waits_ms, 7);
        assert!(here.check().is_ok());
        assert_ne!(here.name, bound(&["account", "other"], &asked).name);
        // The parts are told apart, not run together.
        assert_ne!(bound(&["ab", "c"], &asked).name, bound(&["a", "bc"], &asked).name);
    }

    #[tokio::test]
    async fn the_scope_names_what_runs_inside_it() {
        assert_eq!(current(), None);
        let asked = Submission { name: "n".into(), waits_ms: 0 };
        let inside = scope(Some(asked.clone()), async { current() }).await;
        assert_eq!(inside, Some(asked));
        assert_eq!(scope(None, async { current() }).await, None);
    }
}
