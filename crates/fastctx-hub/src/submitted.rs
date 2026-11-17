//! Questions to the hub that submit work (`fastctx_jobs::submitted`, which
//! says how: the work is recorded with its job and done after, the question
//! stays a bounded while for the outcome, and a named question asked again
//! is done once).
//!
//! What is the hub's own is here. Its jobs live in its shelf database, and
//! so does what a question asks or its work answers when that is too large
//! for a row of them: a change to a large network names many machines and
//! answers for each. It is kept by its hash as a receipt of that database,
//! a little longer than the operation it belongs to stands, and goes with
//! the receipts that have had their time. And where a call between machines fails
//! with a code that says its work is accepted, a question's failure is only
//! words: the hub says so with an answer of its own (`Told::Accepted`),
//! given only to a question that came with a name, which only a machine
//! that knows the answer sends (`View::named_asks`).

use crate::Hub;
use fastctx_jobs::submitted::{Kept, Service};
use fastctx_net::wire::Told;
use fastctx_storage::receipts;
use fastctx_types::MachineName;
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub(crate) use fastctx_jobs::submitted::Family;

/// The receipts that hold what is too large for a row.
const KEPT: &str = "submitted-kept";
/// The most one of them holds: what one answer of the hub holds at most, so
/// that no answer the hub could give before is one it can no longer keep.
const KEPT_BYTES: usize = 16 * 1024 * 1024;
/// How long one is kept: an hour past the day an operation stands settled.
const KEPT_FOR: Duration = Duration::from_secs(25 * 60 * 60);
/// How often receipts that have had their time are let go, and how many at once.
const PRUNED_EVERY: Duration = Duration::from_secs(10 * 60);
const PRUNED_AT_ONCE: usize = 4096;

fn kept_as(name: &[u8; 32]) -> receipts::Key {
    receipts::Key::new(KEPT, "", name.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

/// Lets go, for as long as the hub serves, of the receipts that have had
/// their time: settled operations and what was kept for them. Nothing else
/// prunes the shelf's receipts.
pub(crate) fn upkeep(hub: &Arc<Hub>) {
    let serving = Arc::downgrade(hub);
    fastctx_platform::task::spawn(async move {
        loop {
            let Some(hub) = serving.upgrade() else { return };
            let now = jiff::Timestamp::now().as_millisecond();
            match hub.durable_jobs.database().write(0, move |db| receipts::prune(db, now, PRUNED_AT_ONCE)).await {
                // More may have had their time than one pass lets go.
                Ok(pruned) if pruned == PRUNED_AT_ONCE => continue,
                Ok(_) => {}
                Err(error) => hub.log.line(format_args!("letting go of receipts past their time: {error}")),
            }
            drop(hub);
            tokio::time::sleep(PRUNED_EVERY).await;
        }
    });
}

impl Service for Hub {
    fn told_as(&self) -> String {
        "the hub".into()
    }

    fn jobs(&self) -> &Arc<fastctx_jobs::Jobs> {
        &self.durable_jobs
    }

    fn fresh(&self) -> Result<String, String> {
        let name = fastctx_net::sign::random::<16>().map_err(|error| error.to_string())?;
        Ok(name.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    fn keep(&self, bytes: Vec<u8>) -> Kept<'_, [u8; 32]> {
        Box::pin(async move {
            if bytes.len() > KEPT_BYTES {
                return Err(format!(
                    "the hub keeps at most {} MiB with the work it is asked for; this is {} bytes",
                    KEPT_BYTES / (1024 * 1024),
                    bytes.len()
                ));
            }
            let name: [u8; 32] = Sha256::digest(&bytes).into();
            let (key, now) = (kept_as(&name), jiff::Timestamp::now().as_millisecond());
            self.durable_jobs
                .database()
                .write(bytes.len(), move |db| receipts::put(db, &key, &bytes, KEPT_FOR, true, None, now))
                .await
                .map_err(|error| error.to_string())?;
            Ok(name)
        })
    }

    fn kept(&self, name: [u8; 32]) -> Kept<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            let key = kept_as(&name);
            self.durable_jobs
                .database()
                .read(move |db| receipts::get(db, &key))
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn log(&self, line: std::fmt::Arguments<'_>) {
        self.log.line(line);
    }
}

/// The name a question came with, as the hub knows it: bound to the member
/// that asked, so one member's names are no other's.
pub(crate) fn named(member: &MachineName, submission: &Submission) -> Result<Submission, String> {
    submission.check()?;
    Ok(fastctx_net::submission::bound(&["member", member.as_str()], submission))
}

/// Records what a question of `family` asks for `unit` and stays for what
/// comes of it. `caller` is the question's name as `named` gave it.
pub(crate) async fn submit<C: Serialize, T: DeserializeOwned>(
    hub: &Hub,
    family: &Family,
    unit: &str,
    asked: &C,
    caller: Option<&Submission>,
) -> Result<T, Failure> {
    let told_apart =
        serde_json::to_vec(asked).map_err(|error| Failure::not_run(format!("recording {}: {error}", family.name)))?;
    let make = || async { Ok((unit.to_string(), asked)) };
    fastctx_jobs::submitted::submit(hub, family, &told_apart, caller, make, || {}).await
}

/// Names what does the work of a family's questions. `doing` is given what
/// `submit` was asked with.
pub(crate) fn register<C, T, F, Doing>(hub: &Arc<Hub>, family: Family, doing: F)
where
    C: DeserializeOwned + Send + 'static,
    T: Serialize + Send + 'static,
    F: Fn(Arc<Hub>, C) -> Doing + Send + Sync + 'static,
    Doing: Future<Output = Result<T, Failure>> + Send + 'static,
{
    fastctx_jobs::submitted::register(hub, family, move |hub: Arc<Hub>, asked: C, _: Submission| doing(hub, asked));
}

/// What a question that submitted work is told: what came of the work; that
/// the hub accepted it, when the question came with a name; or why not.
pub(crate) fn told(came: Result<Told, Failure>) -> Result<Told, String> {
    match came {
        Ok(told) => Ok(told),
        Err(failure) if failure.is_accepted() => Ok(Told::Accepted),
        Err(failure) => Err(failure.message),
    }
}
