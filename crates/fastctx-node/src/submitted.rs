//! Calls that submit work.
//!
//! A call whose work takes time does not do it while its caller holds a
//! place. In one transaction it records what was asked as an operation and
//! the job that does it (`fastctx_jobs`), and then stays a bounded while for
//! the outcome: its caller is told what came of the work if that is soon,
//! and that it is accepted if not (`Failure::accepted`). The job does the
//! work after, one of a unit at a time, and settles the operation with its
//! answer.
//!
//! A caller that names its call (`fastctx_types::submission`) asks the same
//! thing again under the same name and has it done once: the second asking
//! finds the operation and is answered with how it stands. That is also how
//! a caller told "accepted" learns the outcome. A call without a name is
//! done once for each time it is asked, as it always was.
//!
//! A family of calls is converted by naming itself (`Family`), registering
//! what does its work (`register`) and having its entrance `submit`. The
//! work must not submit to its own family for its own unit: that job would
//! wait for the one asking for it.

use crate::Node;
use fastctx_jobs::{Job, Kind, Operation, Outcome, Standing, Taken};
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How long an operation's outcome is kept after it settles, for a caller
/// that asks again late: a device that was offline, an answer lost on the way.
const KEPT: Duration = Duration::from_secs(24 * 60 * 60);
/// Tries a job of a family whose work may be done again is given before its
/// operation fails.
const TRIES: u32 = 3;
/// How soon work that waits on another machine's accepted work asks again.
const ASKS_AGAIN: Duration = Duration::from_secs(1);
/// Bytes a transaction here retains beyond what was asked.
const WRITE_BYTES: usize = 1024;

/// One family of submitting calls and how its work is run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Family {
    /// The family's name, which is its job kind: `call/<what>`.
    pub name: &'static str,
    /// How many of the family are done at once; one of a unit at a time.
    pub at_once: usize,
    /// The longest doing one may take; past it the work is dropped.
    pub within: Duration,
    /// The longest a caller is kept for the outcome, under the time its
    /// call is waited for (`fastctx_types::contract`).
    pub stays: Duration,
    /// Whether doing the work again is harmless after a try that did not
    /// finish, as when the node stopped under it. Where it is not, such a
    /// try fails the operation: whether it took effect is not known.
    pub again: bool,
}

/// What is recorded of a call: what it asked, and whom its caller acted as
/// for the records it shares (`crate::record_access`), which the work acts
/// as too.
#[derive(Serialize)]
struct Recording<'a, C> {
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<crate::record_access::Access>,
    asked: &'a C,
}

#[derive(serde::Deserialize)]
struct Recorded<C> {
    #[serde(default)]
    access: Option<crate::record_access::Access>,
    asked: C,
}

/// The operation's name for what was asked: the caller's name with what it
/// asked, so a name used for two different things is two operations.
fn named(name: &str, asked: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update((name.len() as u64).to_le_bytes());
    hash.update(name.as_bytes());
    hash.update(asked);
    hash.finalize()[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn failed(failure: &Failure) -> Standing {
    Standing::Failed(serde_json::to_string(failure).unwrap_or_else(|_| failure.message.clone()))
}

/// Records what a call of `family` asks for `unit` and stays for what comes
/// of it: the work's answer, its failure, or that it is accepted.
pub(crate) async fn submit<C: Serialize, T: DeserializeOwned>(
    node: &Node,
    family: &Family,
    unit: &str,
    asked: &C,
) -> Result<T, Failure> {
    let kind = family.name;
    let recording = Recording { access: crate::record_access::current(), asked };
    let payload =
        serde_json::to_vec(&recording).map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?;
    if payload.len() > fastctx_jobs::PAYLOAD_BYTES {
        return Err(Failure::not_run(format!(
            "{kind} takes at most {} bytes, and this is {}",
            fastctx_jobs::PAYLOAD_BYTES,
            payload.len()
        )));
    }
    let caller = fastctx_net::submission::current();
    let id = match &caller {
        Some(submission) => named(&submission.name, &payload),
        None => fastctx_net::sign::random::<16>()
            .map_err(|error| Failure::not_run(format!("naming {kind}: {error}")))?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    let operation = Operation { kind, unit: unit.to_string(), id: id.clone() };
    let job = Job { kind, key: id, unit: unit.to_string(), payload };
    let accepted = operation.clone();
    let known = node
        .database
        .write(WRITE_BYTES + job.payload.len(), move |db| {
            let known = fastctx_jobs::accept(db, &accepted)?;
            if known.is_none() {
                fastctx_jobs::submit(db, &job)?;
            }
            Ok(known)
        })
        .await
        .map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?;
    if known.is_none() {
        node.durable_jobs.stir(kind);
    }
    // The call has done what a submission does; the rest is its caller
    // staying for the outcome.
    fastctx_platform::call_timing::recorded();
    let stays = match &caller {
        Some(submission) => Duration::from_millis(u64::from(submission.waits_ms)).min(family.stays),
        None => family.stays,
    };
    let standing = match known {
        Some(settled) if settled != Standing::Accepted => Some(settled),
        _ => node
            .durable_jobs
            .settled(&operation, stays)
            .await
            .map_err(|error| Failure::new(format!("reading what came of {kind}: {error}")))?,
    };
    match standing {
        Some(Standing::Done(answer)) => serde_json::from_slice(&answer)
            .map_err(|error| Failure::new(format!("{kind} was done, and its answer cannot be read: {error}"))),
        Some(Standing::Failed(why)) => Err(serde_json::from_str(&why).unwrap_or_else(|_| Failure::new(why))),
        Some(Standing::Accepted) if caller.is_some() => Err(Failure::accepted(format!(
            "{} accepted {kind} and is doing it; asked again the same way, it says what came of it",
            node.machine()
        ))),
        Some(Standing::Accepted) => Err(Failure::new(format!(
            "{} did not finish {kind} within {} s; it is still being done there, and whether it takes effect is not known yet",
            node.machine(),
            stays.as_secs()
        ))),
        None => Err(Failure::new(format!("{} no longer keeps what came of {kind}", node.machine()))),
    }
}

/// Names what does the work of a family's calls, before the node's jobs
/// start. `doing` is given what `submit` was asked with.
pub(crate) fn register<C, T, F, Doing>(node: &Arc<Node>, family: Family, doing: F)
where
    C: DeserializeOwned + Send + 'static,
    T: Serialize + Send + 'static,
    F: Fn(Arc<Node>, C) -> Doing + Send + Sync + 'static,
    Doing: Future<Output = Result<T, Failure>> + Send + 'static,
{
    let jobs = node.durable_jobs.clone();
    let node = Arc::downgrade(node);
    let doing = Arc::new(doing);
    let run = move |taken: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let (node, doing) = (node.upgrade(), doing.clone());
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let operation = Operation { kind: family.name, unit: taken.unit.clone(), id: taken.key.clone() };
            let standing = match work(&node, &family, &taken, doing.as_ref()).await {
                Some(standing) => standing,
                None => return Outcome::Again { after: ASKS_AGAIN },
            };
            let settled = operation.clone();
            let bytes = WRITE_BYTES + taken.payload.len();
            let written =
                node.database.write(bytes, move |db| fastctx_jobs::settle(db, &settled, &standing, KEPT)).await;
            match written {
                Ok(()) => {
                    node.durable_jobs.told(&operation);
                    Outcome::Done
                }
                Err(error) => Outcome::Failed(format!("recording what came of it: {error}")),
            }
        })
    };
    // One try more than the work is given: the last finds it unfinished
    // and settles its operation as failed, where a job set aside would
    // leave its caller waiting.
    let tries = if family.again { TRIES + 1 } else { 2 };
    jobs.register(Kind { name: family.name, at_once: family.at_once, within: family.within, tries }, Arc::new(run));
}

/// Does one job's work and says how its operation stands after; none when
/// it waits on another machine's accepted work and is to ask again.
async fn work<C, T, F, Doing>(node: &Arc<Node>, family: &Family, taken: &Taken, doing: &F) -> Option<Standing>
where
    C: DeserializeOwned,
    T: Serialize,
    F: Fn(Arc<Node>, C) -> Doing,
    Doing: Future<Output = Result<T, Failure>>,
{
    let kind = family.name;
    let unfinished = taken.attempt > 1;
    if unfinished && (!family.again || taken.attempt > TRIES) {
        return Some(failed(&Failure::new(format!(
            "{} began {kind} and did not finish it; whether it took effect is not known",
            node.machine()
        ))));
    }
    let Recorded { access, asked } = match serde_json::from_slice::<Recorded<C>>(&taken.payload) {
        Ok(recorded) => recorded,
        Err(error) => {
            return Some(failed(&Failure::not_run(format!("{kind} as it was recorded cannot be read: {error}"))));
        }
    };
    // What the work asks of other machines is asked under this operation's
    // name, so doing the work again asks them the same thing again. It is
    // accepted work: it waits its turn at a conversation's gates, where a
    // call would be told the conversation is busy.
    let naming = Submission { name: taken.key.clone(), waits_ms: u32::MAX };
    let working = crate::gate_waits::internal(doing(node.clone(), asked));
    let working = crate::record_access::within(access, working);
    match fastctx_net::submission::scope(Some(naming), working).await {
        Ok(answer) => Some(match serde_json::to_vec(&answer) {
            Ok(answer) if answer.len() <= fastctx_jobs::RESULT_BYTES => Standing::Done(answer),
            Ok(answer) => failed(&Failure::new(format!(
                "{kind} was done; its answer of {} bytes is more than is kept, so read what it changed",
                answer.len()
            ))),
            Err(error) => failed(&Failure::new(format!("{kind} was done, and its answer cannot be kept: {error}"))),
        }),
        Err(failure) if failure.is_accepted() && family.again => None,
        Err(failure) if failure.is_accepted() => {
            Some(failed(&Failure::new(format!("{}; whether {kind} takes effect is not known yet", failure.message))))
        }
        Err(failure) => Some(failed(&failure)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_used_for_two_things_is_two_operations() {
        assert_eq!(named("n", b"asked"), named("n", b"asked"));
        assert_ne!(named("n", b"asked"), named("n", b"other"));
        assert_ne!(named("n", b"asked"), named("m", b"asked"));
        assert_ne!(named("ab", b"c"), named("a", b"bc"));
        assert_eq!(named("n", b"asked").len(), 32);
    }

    #[test]
    fn a_failure_is_kept_whole() {
        let Standing::Failed(kept) = failed(&Failure::refused("no")) else { panic!("a failure") };
        let read: Failure = serde_json::from_str(&kept).unwrap();
        assert!(read.is_refused() && read.not_run);
    }
}
