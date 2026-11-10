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
//!
//! The work is never cut short: as a call's work went on to its end past
//! the time its caller waited, a family's work goes on past `within`, the
//! unit kept for it, and only its operation is settled meanwhile as still
//! being done. What was asked and what was answered may be of any size:
//! past a few dozen KiB each is kept as an object.

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
const SETTLED_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// Tries a job of a family whose work may be done again is given before its
/// operation fails.
const TRIES: u32 = 3;
/// How soon work that waits on another machine's accepted work asks again.
const ASKS_AGAIN: Duration = Duration::from_secs(1);
/// How long it goes on asking: as long as a client goes on asking a machine.
const ASKS_FOR: Duration = Duration::from_secs(60 * 60);
/// Bytes a transaction here retains beyond what was asked.
const WRITE_BYTES: usize = 1024;
/// What was asked is a job's payload up to this size; a larger one is kept
/// as an object, which the payload names.
const INLINE_BYTES: usize = 48 * 1024;
/// The first byte of a payload, or of an answer, that names the object
/// holding it; what it holds itself is JSON, which never begins so.
const KEPT: u8 = b'@';
/// The longest the queue lets one job of a family run. The work is not cut
/// short at `Family::within`; this only keeps a job that never ends from
/// holding its place for ever.
const LONGEST: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One family of submitting calls and how its work is run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Family {
    /// The family's name, which is its job kind: `call/<what>`.
    pub name: &'static str,
    /// How many of the family are done at once; one of a unit at a time.
    pub at_once: usize,
    /// How long doing one is expected to take at most. Past it the work
    /// goes on, and those who wait are told that whether it takes effect
    /// is not known yet.
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

/// One call of another machine that it may answer as accepted, made again
/// under the name it was first made with until that machine says what came
/// of the work, or the hour is up.
pub(crate) struct Asking {
    began: tokio::time::Instant,
}

impl Asking {
    pub(crate) fn begin() -> Self {
        Self { began: tokio::time::Instant::now() }
    }

    /// Takes a failure the other machine answered. When it only says the
    /// work is accepted, this waits a moment and the call is made again;
    /// any other failure is the answer. An acceptance an hour old is told
    /// as work whose effect is not known, not as one to go on asking after.
    pub(crate) async fn accepted(&self, failure: Failure) -> Result<(), Failure> {
        if !failure.is_accepted() {
            return Err(failure);
        }
        if self.began.elapsed() >= ASKS_FOR {
            return Err(Failure::new(format!(
                "{} An hour on, it has not said what came of it; whether it takes effect is not known",
                failure.message
            )));
        }
        tokio::time::sleep(ASKS_AGAIN).await;
        Ok(())
    }

    /// What the other machine answered, when that is its answer; none when
    /// the call is to be made again (`accepted`).
    pub(crate) async fn settled<T>(&self, reply: Result<T, Failure>) -> Option<Result<T, Failure>> {
        match reply {
            Err(failure) => self.accepted(failure).await.err().map(Err),
            reply => Some(reply),
        }
    }
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
    let payload = if payload.len() > INLINE_BYTES { keep(node, kind, payload).await? } else { payload };
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
    // staying for the outcome, which takes no place from the calls after it.
    fastctx_platform::call_timing::recorded();
    fastctx_net::link::admission::stays();
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
        Some(Standing::Done(answer)) => {
            let unread = |why: String| Failure::new(format!("{kind} was done, and its answer cannot be read: {why}"));
            let answer =
                kept_of(node, &answer).await.map_err(unread)?.ok_or_else(|| unread("it is no longer kept".into()))?;
            serde_json::from_slice(&answer).map_err(|error| unread(error.to_string()))
        }
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

/// The payload that names the object holding what was asked.
fn naming(hash: &[u8; 32]) -> Vec<u8> {
    let mut payload = vec![KEPT];
    payload.extend(hash.iter().flat_map(|byte| format!("{byte:02x}").into_bytes()));
    payload
}

/// The object a payload names: none when the payload is what was asked
/// itself, and none within when it names an object badly.
fn named_by(payload: &[u8]) -> Option<Option<[u8; 32]>> {
    let (&KEPT, name) = payload.split_first()? else { return None };
    let mut hash = [0u8; 32];
    let read = name.len() == 64
        && name.chunks(2).zip(hash.iter_mut()).all(|(pair, byte)| {
            let read = std::str::from_utf8(pair).ok().and_then(|pair| u8::from_str_radix(pair, 16).ok());
            read.map(|read| *byte = read).is_some()
        });
    Some(read.then_some(hash))
}

/// Keeps what was asked, or answered, as an object and gives what names
/// it. The object is put before what reads it is recorded, and is reclaimed
/// a day after (`crate::content_upkeep`): work asked for is done within the
/// day, and an answer is kept as long as its operation is.
async fn keep(node: &Node, kind: &'static str, asked: Vec<u8>) -> Result<Vec<u8>, Failure> {
    let content = node.content()?.clone();
    let kept = fastctx_platform::task::spawn_blocking(move || content.objects().put(&asked))
        .await
        .map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?
        .map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?;
    Ok(naming(&kept.hash))
}

/// What was asked or answered, as a payload or a settled answer holds or
/// names it; none when the object that held it is no longer kept.
async fn kept_of(node: &Node, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let hash = match named_by(payload) {
        None => return Ok(Some(payload.to_vec())),
        Some(None) => return Err("the payload names no object".into()),
        Some(Some(hash)) => hash,
    };
    let content = node.content().map_err(|failure| failure.message)?.clone();
    fastctx_platform::task::spawn_blocking(move || -> Result<Option<Vec<u8>>, String> {
        let objects = content.objects();
        let Some(size) = objects.size(&hash).map_err(|error| error.to_string())? else { return Ok(None) };
        let mut asked = Vec::with_capacity(size as usize);
        while (asked.len() as u64) < size {
            match objects.read(&hash, asked.len() as u64, fastctx_content::READ_BYTES) {
                Ok(Some(piece)) if !piece.is_empty() => asked.extend_from_slice(&piece),
                Ok(_) => return Ok(None),
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(Some(asked))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Takes back what a family was asked for `unit` and has not begun, as
/// when what it would have worked on is stopped: each such operation is
/// settled as not done, for `why`. Work that has begun goes on.
pub(crate) async fn withdraw(node: &Node, family: &Family, unit: &str, why: &str) -> Result<usize, Failure> {
    let kind = family.name;
    let (unit, reason) = (unit.to_string(), failed(&Failure::not_run(why)));
    let withdrawn = unit.clone();
    let keys = node
        .database
        .write(WRITE_BYTES, move |db| {
            let keys = fastctx_jobs::withdraw(db, kind, &withdrawn)?;
            for key in &keys {
                let operation = Operation { kind, unit: withdrawn.clone(), id: key.clone() };
                fastctx_jobs::settle(db, &operation, &reason, SETTLED_FOR)?;
            }
            Ok(keys)
        })
        .await
        .map_err(|error| Failure::new(format!("withdrawing {kind}: {error}")))?;
    for key in &keys {
        node.durable_jobs.told(&Operation { kind, unit: unit.clone(), id: key.clone() });
    }
    Ok(keys.len())
}

/// Settles an operation and tells those who wait for it.
async fn settle(node: &Node, operation: &Operation, standing: Standing) -> Result<(), String> {
    let settled = operation.clone();
    let bytes = WRITE_BYTES + operation.id.len() + operation.unit.len();
    node.database
        .write(bytes, move |db| fastctx_jobs::settle(db, &settled, &standing, SETTLED_FOR))
        .await
        .map_err(|error| error.to_string())?;
    node.durable_jobs.told(operation);
    Ok(())
}

/// Names what does the work of a family's calls. `doing` is given what
/// `submit` was asked with.
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
            let kind = family.name;
            let operation = Operation { kind, unit: taken.unit.clone(), id: taken.key.clone() };
            // The work runs as a task of its own: nothing that gives up on
            // this job, not the queue and not the node handing over, cuts
            // it short between two of its steps.
            let mut working = fastctx_platform::task::spawn(work(node.clone(), family, taken, doing));
            let came = match tokio::time::timeout(family.within, &mut working).await {
                Ok(came) => came,
                Err(_) => {
                    let unknown = failed(&Failure::new(format!(
                        "{} did not finish {kind} within {} s; it is still being done there, and whether it takes effect is not known yet",
                        node.machine(),
                        family.within.as_secs()
                    )));
                    if let Err(error) = settle(&node, &operation, unknown).await {
                        node.log.line(format_args!("recording that {kind} goes on: {error}"));
                    }
                    working.await
                }
            };
            let standing = match came {
                Ok(Some(standing)) => standing,
                Ok(None) => return Outcome::Again { after: ASKS_AGAIN },
                Err(error) => failed(&Failure::new(format!("{kind} ended unexpectedly: {error}"))),
            };
            match settle(&node, &operation, standing).await {
                Ok(()) => Outcome::Done,
                Err(error) => Outcome::Failed(format!("recording what came of it: {error}")),
            }
        })
    };
    // One try more than the work is given: the last finds it unfinished
    // and settles its operation as failed, where a job set aside would
    // leave its caller waiting.
    let tries = if family.again { TRIES + 1 } else { 2 };
    jobs.register(Kind { name: family.name, at_once: family.at_once, within: LONGEST, tries }, Arc::new(run));
}

/// Does one job's work and says how its operation stands after; none when
/// it waits on another machine's accepted work and is to ask again.
async fn work<C, T, F, Doing>(node: Arc<Node>, family: Family, taken: Taken, doing: Arc<F>) -> Option<Standing>
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
    let unreadable =
        |why: String| Some(failed(&Failure::not_run(format!("{kind} as it was recorded cannot be read: {why}"))));
    let recorded = match kept_of(&node, &taken.payload).await {
        Ok(Some(recorded)) => recorded,
        Ok(None) => return unreadable("what was asked is no longer kept".into()),
        Err(why) => return unreadable(why),
    };
    let Recorded { access, asked } = match serde_json::from_slice::<Recorded<C>>(&recorded) {
        Ok(recorded) => recorded,
        Err(error) => return unreadable(error.to_string()),
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
            Ok(answer) if answer.len() <= INLINE_BYTES => Standing::Done(answer),
            // A large answer is kept beside its operation, for as long.
            Ok(answer) => match keep(&node, kind, answer).await {
                Ok(named) => Standing::Done(named),
                Err(failure) => failed(&Failure::new(format!(
                    "{kind} was done, and its answer could not be kept: {}",
                    failure.message
                ))),
            },
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
    fn a_payload_is_what_was_asked_or_names_its_object() {
        let hash: [u8; 32] = std::array::from_fn(|at| at as u8 * 7);
        assert_eq!(named_by(&naming(&hash)), Some(Some(hash)));
        assert_eq!(named_by(br#"{"asked":1}"#), None);
        assert_eq!(named_by(b""), None);
        assert_eq!(named_by(b"@short"), Some(None));
        assert_eq!(named_by(&[&b"@"[..], &[b'g'; 64][..]].concat()), Some(None));
    }

    #[test]
    fn a_failure_is_kept_whole() {
        let Standing::Failed(kept) = failed(&Failure::refused("no")) else { panic!("a failure") };
        let read: Failure = serde_json::from_str(&kept).unwrap();
        assert!(read.is_refused() && read.not_run);
    }
}
