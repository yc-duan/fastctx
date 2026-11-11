//! Calls that submit work.
//!
//! A call whose work takes time does not do it while its caller holds a
//! place. In one transaction it records what was asked as an operation and
//! the job that does it, and then stays a bounded while for the outcome:
//! its caller is told what came of the work if that is soon, and that it is
//! accepted if not (`Failure::accepted`). The job does the work after, one
//! of a unit at a time in the order asked for, and settles the operation
//! with its answer.
//!
//! A caller that names its call (`fastctx_types::submission`) asks the same
//! thing again under the same name and has it done once: the second asking
//! finds the operation and is answered with how it stands. That is also how
//! a caller told "accepted" learns the outcome. A call without a name is
//! done once for each time it is asked.
//!
//! A family of calls is converted by naming itself (`Family`), registering
//! what does its work (`register`) and having its entrance `submit`. The
//! work must not submit to its own family for its own unit: that job would
//! wait for the one asking for it.
//!
//! The work is never cut short: a family's work goes on past `within`, the
//! unit kept for it, and only its operation is settled meanwhile as still
//! being done. What was asked and what was answered may be of any size the
//! service keeps (`Service::keep`): past a few dozen KiB each is kept apart
//! and named.
//!
//! This is the same for every service that answers calls, a machine's node
//! or a hub: each says where its jobs live and how it keeps what is large
//! (`Service`), and wraps `submit` and `register` with what is its own, such
//! as whom the work acts as.

use crate::{Job, Jobs, Kind, Operation, Outcome, Standing, Taken};
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
/// How soon work that waits on another service's accepted work asks again.
const ASKS_AGAIN: Duration = Duration::from_secs(1);
/// How long it goes on asking: as long as a client goes on asking a machine.
const ASKS_FOR: Duration = Duration::from_secs(60 * 60);
/// Bytes a transaction here retains beyond what was asked.
const WRITE_BYTES: usize = 1024;
/// The seconds waited before each further try at recording what came of
/// work that is done.
const RECORDS_AGAIN: [u64; 5] = [1, 2, 4, 8, 16];
/// What was asked is a job's payload up to this size; a larger one is kept
/// apart, and the payload names it.
const INLINE_BYTES: usize = 48 * 1024;
/// The first byte of a payload, or of an answer, that names what is kept
/// apart; what it holds itself is JSON, which never begins so.
const KEPT: u8 = b'@';
/// The longest the queue lets one job of a family run. The work is not cut
/// short at `Family::within`; this only keeps a job that never ends from
/// holding its place for ever.
const LONGEST: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One family of submitting calls and how its work is run.
#[derive(Clone, Copy, Debug)]
pub struct Family {
    /// The family's name, which is its job kind: `call/<what>`.
    pub name: &'static str,
    /// How many of the family are done at once; one of a unit at a time.
    pub at_once: usize,
    /// How long doing one is expected to take at most. Past it the work
    /// goes on, and those who wait are told that whether it takes effect
    /// is not known yet.
    pub within: Duration,
    /// The longest a caller is kept for the outcome, under the time its
    /// call is waited for.
    pub stays: Duration,
    /// Whether doing the work again is harmless after a try that did not
    /// finish, as when the service stopped under it. Where it is not, such
    /// a try fails the operation: whether it took effect is not known.
    pub again: bool,
}

/// What `Service::keep` and `Service::kept` answer with, in time.
pub type Kept<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// A service whose calls submit work: where its jobs and operations live,
/// and how it keeps what is too large for a row of them.
pub trait Service: Send + Sync + 'static {
    /// What callers are told the service is: a machine's name, a hub's.
    fn told_as(&self) -> String;
    /// Its jobs, in the database its operations live in too.
    fn jobs(&self) -> &Arc<Jobs>;
    /// A name for a call that came without one, which no other has.
    fn fresh(&self) -> Result<String, String>;
    /// Keeps what was asked, or answered, apart and gives the 32 bytes that
    /// name it. It is kept before what reads it is recorded, a day at least
    /// for what was asked and as long as its operation for an answer. A
    /// service that keeps nothing apart refuses, and its calls stay small.
    fn keep(&self, bytes: Vec<u8>) -> Kept<'_, [u8; 32]>;
    /// What was kept under a name; none when it is no longer kept.
    fn kept(&self, name: [u8; 32]) -> Kept<'_, Option<Vec<u8>>>;
    /// Says the service begins work it accepted. What it gives is held
    /// until the work has ended and its operation is settled, as for a call
    /// being answered: a service that hands itself on to a successor does
    /// not do so under it. None when the service takes on no more work now,
    /// being on its way out: the job waits for whoever serves next.
    fn begins(&self) -> Option<Box<dyn Send>> {
        Some(Box::new(()))
    }
    /// Says what went wrong where no caller is there to be told.
    fn log(&self, line: std::fmt::Arguments<'_>);
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

/// The operation of a family that goes by `id`. It is known by its family
/// and that name alone. The unit its job runs for is no part of it: the
/// unit says whose turn the job waits for, and may come of what the call's
/// entrance resolved, which a later asking under the same name would
/// resolve otherwise; that asking is still this operation.
fn operation(kind: &'static str, id: &str) -> Operation {
    Operation { kind, unit: String::new(), id: id.to_string() }
}

fn failed(failure: &Failure) -> Standing {
    Standing::Failed(serde_json::to_string(failure).unwrap_or_else(|_| failure.message.clone()))
}

/// One call of another service that it may answer as accepted, made again
/// under the name it was first made with until that service says what came
/// of the work, or the hour is up.
pub struct Asking {
    began: tokio::time::Instant,
}

impl Asking {
    pub fn begin() -> Self {
        Self { began: tokio::time::Instant::now() }
    }

    /// Takes a failure the other service answered. When it only says the
    /// work is accepted, this waits a moment and the call is made again;
    /// any other failure is the answer. An acceptance an hour old is told
    /// as work whose effect is not known, not as one to go on asking after.
    pub async fn accepted(&self, failure: Failure) -> Result<(), Failure> {
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

    /// What the other service answered, when that is its answer; none when
    /// the call is to be made again (`accepted`).
    pub async fn settled<T>(&self, reply: Result<T, Failure>) -> Option<Result<T, Failure>> {
        match reply {
            Err(failure) => self.accepted(failure).await.err().map(Err),
            reply => Some(reply),
        }
    }
}

/// Records what a call of `family` asks and stays for what comes of it: the
/// work's answer, its failure, or that it is accepted.
///
/// `asked` is what was asked, as bytes: with `caller`, the name the call
/// came with bound to who asked, it tells this asking from another, and
/// nothing else does. A call that is an operation already is answered from
/// it. For one that is not, `make` gives the unit whose turn its job waits
/// for and what the work is given: what was asked, and whatever the service
/// keeps with it, such as whom the caller acted as and what its entrance
/// made of what the call left to how things stood. So what `make` does,
/// and may fail at or come out otherwise by, is done once for an operation,
/// as it is first asked. `recorded` is called once the call has done what a
/// submission does, before it stays.
pub async fn submit<S, C, T, Making>(
    service: &S,
    family: &Family,
    asked: &[u8],
    caller: Option<&Submission>,
    make: impl FnOnce() -> Making,
    recorded: impl FnOnce(),
) -> Result<T, Failure>
where
    S: Service,
    C: Serialize,
    T: DeserializeOwned,
    Making: Future<Output = Result<(String, C), Failure>>,
{
    let kind = family.name;
    let unrecorded = |error: fastctx_storage::Error| Failure::not_run(format!("recording {kind}: {error}"));
    let id = match caller {
        Some(submission) => named(&submission.name, asked),
        None => service.fresh().map_err(|error| Failure::not_run(format!("naming {kind}: {error}")))?,
    };
    let operation = operation(kind, &id);
    let known = match caller {
        Some(_) => standing_of(service, &operation).await.map_err(unrecorded)?,
        // A name just made is no operation yet.
        None => None,
    };
    let made = match known {
        Some(known) => Err(known),
        None => match make().await {
            Ok(made) => Ok(made),
            // A call under the same name may have been recorded while this
            // one was being made: then that is the operation, whatever
            // became of making this one.
            Err(failure) => match caller {
                Some(_) => Err(standing_of(service, &operation).await.ok().flatten().ok_or(failure)?),
                None => return Err(failure),
            },
        },
    };
    let known = match made {
        Err(known) => Some(known),
        Ok((unit, recording)) => {
            let payload = serde_json::to_vec(&recording)
                .map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?;
            let payload = if payload.len() > INLINE_BYTES { keep(service, kind, payload).await? } else { payload };
            let job = Job { kind, key: id, unit, payload };
            let accepted = operation.clone();
            let known = service
                .jobs()
                .database()
                .write(WRITE_BYTES + job.payload.len(), move |db| {
                    let known = crate::accept(db, &accepted)?;
                    if known.is_none() {
                        crate::submit(db, &job)?;
                    }
                    Ok(known)
                })
                .await
                .map_err(unrecorded)?;
            if known.is_none() {
                service.jobs().stir(kind);
            }
            known
        }
    };
    // The call has done what a submission does; the rest is its caller
    // staying for the outcome.
    recorded();
    let stays = match caller {
        Some(submission) => Duration::from_millis(u64::from(submission.waits_ms)).min(family.stays),
        None => family.stays,
    };
    let standing = match known {
        Some(settled) if settled != Standing::Accepted => Some(settled),
        _ => service
            .jobs()
            .settled(&operation, stays)
            .await
            .map_err(|error| Failure::new(format!("reading what came of {kind}: {error}")))?,
    };
    match standing {
        Some(Standing::Done(answer)) => {
            let unread = |why: String| Failure::new(format!("{kind} was done, and its answer cannot be read: {why}"));
            let answer = kept_of(service, &answer)
                .await
                .map_err(unread)?
                .ok_or_else(|| unread("it is no longer kept".into()))?;
            serde_json::from_slice(&answer).map_err(|error| unread(error.to_string()))
        }
        Some(Standing::Failed(why)) => Err(serde_json::from_str(&why).unwrap_or_else(|_| Failure::new(why))),
        Some(Standing::Accepted) if caller.is_some() => Err(Failure::accepted(format!(
            "{} accepted {kind} and is doing it; asked again the same way, it says what came of it",
            service.told_as()
        ))),
        Some(Standing::Accepted) => Err(Failure::new(format!(
            "{} did not finish {kind} within {} s; it is still being done there, and whether it takes effect is not known yet",
            service.told_as(),
            stays.as_secs()
        ))),
        None => Err(Failure::new(format!("{} no longer keeps what came of {kind}", service.told_as()))),
    }
}

/// The payload that names what is kept apart.
fn naming(name: &[u8; 32]) -> Vec<u8> {
    let mut payload = vec![KEPT];
    payload.extend(name.iter().flat_map(|byte| format!("{byte:02x}").into_bytes()));
    payload
}

/// What a payload names as kept apart: none when the payload is what was
/// asked itself, and none within when it names badly.
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

/// Keeps what was asked, or answered, apart and gives the payload that
/// names it.
async fn keep<S: Service>(service: &S, kind: &'static str, bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    let name = service.keep(bytes).await.map_err(|error| Failure::not_run(format!("recording {kind}: {error}")))?;
    Ok(naming(&name))
}

/// What was asked or answered, as a payload or a settled answer holds or
/// names it; none when what it names is no longer kept.
async fn kept_of<S: Service>(service: &S, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
    match named_by(payload) {
        None => Ok(Some(payload.to_vec())),
        Some(None) => Err("the payload names nothing kept".into()),
        Some(Some(name)) => service.kept(name).await,
    }
}

/// Takes back what a family was asked for `unit` and has not begun, as
/// when what it would have worked on is stopped: each such operation is
/// settled as not done, for `why`. Work that has begun goes on.
pub async fn withdraw<S: Service>(service: &S, family: &Family, unit: &str, why: &str) -> Result<usize, Failure> {
    let kind = family.name;
    let (withdrawn, reason) = (unit.to_string(), failed(&Failure::not_run(why)));
    let keys = service
        .jobs()
        .database()
        .write(WRITE_BYTES, move |db| {
            let keys = crate::withdraw(db, kind, &withdrawn)?;
            for key in &keys {
                crate::settle(db, &operation(kind, key), &reason, SETTLED_FOR)?;
            }
            Ok(keys)
        })
        .await
        .map_err(|error| Failure::new(format!("withdrawing {kind}: {error}")))?;
    for key in &keys {
        service.jobs().told(&operation(kind, key));
    }
    Ok(keys.len())
}

/// How an operation stands; none when nothing was recorded under its name.
async fn standing_of<S: Service>(
    service: &S,
    operation: &Operation,
) -> Result<Option<Standing>, fastctx_storage::Error> {
    let asked = operation.clone();
    service.jobs().database().read(move |db| crate::standing(db, &asked)).await
}

/// Settles an operation and tells those who wait for it.
async fn settle<S: Service>(service: &S, operation: &Operation, standing: Standing) -> Result<(), String> {
    let settled = operation.clone();
    let bytes = WRITE_BYTES + operation.id.len() + operation.unit.len();
    service
        .jobs()
        .database()
        .write(bytes, move |db| crate::settle(db, &settled, &standing, SETTLED_FOR))
        .await
        .map_err(|error| error.to_string())?;
    service.jobs().told(operation);
    Ok(())
}

/// Names what does the work of a family's calls. `doing` is given what
/// `submit` was asked with, and the name under which the work asks other
/// services for theirs: the operation's own, so doing the work again asks
/// them the same thing again.
pub fn register<S, C, T, F, Doing>(service: &Arc<S>, family: Family, doing: F)
where
    S: Service,
    C: DeserializeOwned + Send + 'static,
    T: Serialize + Send + 'static,
    F: Fn(Arc<S>, C, Submission) -> Doing + Send + Sync + 'static,
    Doing: Future<Output = Result<T, Failure>> + Send + 'static,
{
    let jobs = service.jobs().clone();
    let service = Arc::downgrade(service);
    let doing = Arc::new(doing);
    let run = move |taken: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let (service, doing) = (service.upgrade(), doing.clone());
        Box::pin(async move {
            let Some(service) = service else { return Outcome::Failed("the service is stopping".into()) };
            let Some(_held) = service.begins() else { return Outcome::Again { after: ASKS_AGAIN } };
            let kind = family.name;
            let operation = operation(kind, &taken.key);
            // An earlier attempt may have settled the operation and ended
            // before its job was done with. What it recorded stands: this
            // attempt must not say over a result that it does not know. Work
            // that is done again where needed may still better a failure.
            if taken.attempt > 1 {
                match standing_of(&*service, &operation).await {
                    Ok(Some(Standing::Done(_))) => return Outcome::Done,
                    Ok(Some(Standing::Failed(_))) if !family.again => return Outcome::Done,
                    Ok(_) => {}
                    Err(error) => return Outcome::Failed(format!("reading how {kind} stands: {error}")),
                }
            }
            // The work runs as a task of its own: nothing that gives up on
            // this job, not the queue and not the service handing over, cuts
            // it short between two of its steps.
            let mut working = fastctx_platform::task::spawn(work(service.clone(), family, taken, doing));
            let came = match tokio::time::timeout(family.within, &mut working).await {
                Ok(came) => came,
                Err(_) => {
                    let unknown = failed(&Failure::new(format!(
                        "{} did not finish {kind} within {} s; it is still being done there, and whether it takes effect is not known yet",
                        service.told_as(),
                        family.within.as_secs()
                    )));
                    if let Err(error) = settle(&*service, &operation, unknown).await {
                        service.log(format_args!("recording that {kind} goes on: {error}"));
                    }
                    working.await
                }
            };
            let standing = match came {
                Ok(Some(standing)) => standing,
                Ok(None) => return Outcome::Again { after: ASKS_AGAIN },
                Err(error) => failed(&Failure::new(format!("{kind} ended unexpectedly: {error}"))),
            };
            // What came of the work is known only here. Its record is tried
            // again before the job gives it up: another attempt of work that
            // is not done twice could only say that it does not know.
            let mut waits = RECORDS_AGAIN.into_iter();
            loop {
                let error = match settle(&*service, &operation, standing.clone()).await {
                    Ok(()) => return Outcome::Done,
                    Err(error) => error,
                };
                let Some(seconds) = waits.next() else {
                    return Outcome::Failed(format!("recording what came of it: {error}"));
                };
                service.log(format_args!("recording what came of {kind}; trying again in {seconds} s: {error}"));
                tokio::time::sleep(Duration::from_secs(seconds)).await;
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
/// it waits on another service's accepted work and is to ask again.
async fn work<S, C, T, F, Doing>(service: Arc<S>, family: Family, taken: Taken, doing: Arc<F>) -> Option<Standing>
where
    S: Service,
    C: DeserializeOwned,
    T: Serialize,
    F: Fn(Arc<S>, C, Submission) -> Doing,
    Doing: Future<Output = Result<T, Failure>>,
{
    let kind = family.name;
    let unfinished = taken.attempt > 1;
    if unfinished && (!family.again || taken.attempt > TRIES) {
        return Some(failed(&Failure::new(format!(
            "{} began {kind} and did not finish it; whether it took effect is not known",
            service.told_as()
        ))));
    }
    let unreadable =
        |why: String| Some(failed(&Failure::not_run(format!("{kind} as it was recorded cannot be read: {why}"))));
    let recorded = match kept_of(&*service, &taken.payload).await {
        Ok(Some(recorded)) => recorded,
        Ok(None) => return unreadable("what was asked is no longer kept".into()),
        Err(why) => return unreadable(why),
    };
    let asked = match serde_json::from_slice::<C>(&recorded) {
        Ok(asked) => asked,
        Err(error) => return unreadable(error.to_string()),
    };
    let naming = Submission { name: taken.key.clone(), waits_ms: u32::MAX };
    match doing(service.clone(), asked, naming).await {
        Ok(answer) => Some(match serde_json::to_vec(&answer) {
            Ok(answer) if answer.len() <= INLINE_BYTES => Standing::Done(answer),
            // A large answer is kept beside its operation, for as long.
            Ok(answer) => match keep(&*service, kind, answer).await {
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
    fn a_payload_is_what_was_asked_or_names_what_is_kept() {
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
