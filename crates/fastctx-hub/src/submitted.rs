//! Questions to the hub that submit work (`fastctx_jobs::submitted`, which
//! says how: the work is recorded with its job and done after, the question
//! stays a bounded while for the outcome, and a named question asked again
//! is done once).
//!
//! What is the hub's own is here. Its jobs live in its shelf database. It
//! keeps nothing apart, so what a question asks and what its work answers
//! are a few dozen KiB at most. And where a call between machines fails
//! with a code that says its work is accepted, a question's failure is only
//! words: the hub says so with an answer of its own (`Told::Accepted`),
//! given only to a question that came with a name, which only a machine
//! that knows the answer sends (`View::named_asks`).

use crate::Hub;
use fastctx_jobs::submitted::{Kept, Service};
use fastctx_net::wire::Told;
use fastctx_types::MachineName;
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::sync::Arc;

pub(crate) use fastctx_jobs::submitted::Family;

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

    fn keep(&self, _: Vec<u8>) -> Kept<'_, [u8; 32]> {
        Box::pin(async { Err("the hub records no more than a few dozen KiB with the work it is asked for".into()) })
    }

    fn kept(&self, _: [u8; 32]) -> Kept<'_, Option<Vec<u8>>> {
        Box::pin(async { Ok(None) })
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
    fastctx_jobs::submitted::submit(hub, family, unit, asked, &told_apart, caller, || {}).await
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
