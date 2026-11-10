//! Calls of this machine that submit work (`fastctx_jobs::submitted`, which
//! says how: the work is recorded with its job and done after, the call
//! stays a bounded while for the outcome, and a named call asked again is
//! done once).
//!
//! What is the node's own is here: its jobs live in its state database;
//! what is too large for a row is an object of its content database; a
//! call's work acts as whom its caller acted as for the records it shares,
//! waits its turn at a conversation's gates as accepted work does, asks
//! other machines under its operation's name, and keeps the node from
//! handing the World on until it has ended.

use crate::Node;
use fastctx_jobs::submitted::{Kept, Service};
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::sync::Arc;

pub(crate) use fastctx_jobs::submitted::{Asking, Family};

/// What is recorded of a call: what it asked; whom its caller acted as for
/// the records it shares (`crate::record_access`), which the work acts as
/// too; and what its entrance resolved, where it resolved anything.
#[derive(Serialize)]
struct Recording<'a, C, R> {
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<crate::record_access::Access>,
    asked: &'a C,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved: Option<R>,
}

#[derive(serde::Deserialize)]
struct Recorded<C, R> {
    #[serde(default)]
    access: Option<crate::record_access::Access>,
    asked: C,
    #[serde(default = "Option::default")]
    resolved: Option<R>,
}

impl Service for Node {
    fn told_as(&self) -> String {
        self.machine().to_string()
    }

    fn jobs(&self) -> &Arc<fastctx_jobs::Jobs> {
        &self.durable_jobs
    }

    fn fresh(&self) -> Result<String, String> {
        let name = fastctx_net::sign::random::<16>().map_err(|error| error.to_string())?;
        Ok(name.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// An object of the content database, reclaimed a day after it is put
    /// unless a record holds it (`crate::content_upkeep`): work asked for
    /// is done within the day.
    fn keep(&self, bytes: Vec<u8>) -> Kept<'_, [u8; 32]> {
        Box::pin(async move {
            let content = self.content().map_err(|failure| failure.message)?.clone();
            let kept = fastctx_platform::task::spawn_blocking(move || content.objects().put(&bytes))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            Ok(kept.hash)
        })
    }

    fn kept(&self, name: [u8; 32]) -> Kept<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            let content = self.content().map_err(|failure| failure.message)?.clone();
            fastctx_platform::task::spawn_blocking(move || -> Result<Option<Vec<u8>>, String> {
                let objects = content.objects();
                let Some(size) = objects.size(&name).map_err(|error| error.to_string())? else { return Ok(None) };
                let mut kept = Vec::with_capacity(size as usize);
                while (kept.len() as u64) < size {
                    match objects.read(&name, kept.len() as u64, fastctx_content::READ_BYTES) {
                        Ok(Some(piece)) if !piece.is_empty() => kept.extend_from_slice(&piece),
                        Ok(_) => return Ok(None),
                        Err(error) => return Err(error.to_string()),
                    }
                }
                Ok(Some(kept))
            })
            .await
            .map_err(|error| error.to_string())?
        })
    }

    /// The node does not hand the World on to its successor under work it
    /// accepted, as it does not under a call it answers; on its way out it
    /// begins none, and its successor does the work.
    fn begins(&self) -> Option<Box<dyn Send>> {
        self.handoff.call().map(|busy| Box::new(busy) as Box<dyn Send>)
    }

    fn log(&self, line: std::fmt::Arguments<'_>) {
        self.log.line(line);
    }
}

/// Records what a call of `family` asks for `unit` and stays for what comes
/// of it: the work's answer, its failure, or that it is accepted.
pub(crate) async fn submit<C: Serialize, T: DeserializeOwned>(
    node: &Node,
    family: &Family,
    unit: &str,
    asked: &C,
) -> Result<T, Failure> {
    record(node, family, asked, || async { Ok((unit.to_string(), None::<()>)) }).await
}

/// `submit` for a call that leaves something to how things stand as it is
/// asked: which town, which account. `resolved` is what its entrance made
/// of that. It is recorded with the operation and given to the work, which
/// does what was asked of what it meant then and not of what it would mean
/// when the work's turn comes; and it is no part of what tells this asking
/// from another, so the call asked again is the same operation though it
/// would resolve otherwise by then. `unit` is not part of that either, and
/// is best the thing resolved to: the work waits its turn behind the other
/// work on that thing.
///
/// An entrance that calls this has resolved before it knows whether the
/// call is an operation already; where resolving can fail by then, or the
/// entrance should not wait for it twice, `submit_resolving` resolves only
/// what is new.
#[expect(
    dead_code,
    reason = "the first family whose entrance resolves what its calls leave implicit is not merged yet"
)]
pub(crate) async fn submit_resolved<C: Serialize, R: Serialize, T: DeserializeOwned>(
    node: &Node,
    family: &Family,
    unit: &str,
    asked: &C,
    resolved: &R,
) -> Result<T, Failure> {
    record(node, family, asked, || async { Ok((unit.to_string(), Some(resolved))) }).await
}

/// `submit_resolved` that resolves only a call that is not an operation
/// yet: `resolve` gives the unit and what the entrance made of what the
/// call leaves implicit. A call asked again under its name is answered
/// from the operation it is, though resolving it now would fail or come
/// out otherwise.
#[expect(dead_code, reason = "the families whose entrances resolve are moved to it as they are merged")]
pub(crate) async fn submit_resolving<C, R, T, Resolving>(
    node: &Node,
    family: &Family,
    asked: &C,
    resolve: impl FnOnce() -> Resolving,
) -> Result<T, Failure>
where
    C: Serialize,
    R: Serialize,
    T: DeserializeOwned,
    Resolving: Future<Output = Result<(String, R), Failure>>,
{
    record(node, family, asked, || async {
        let (unit, resolved) = resolve().await?;
        Ok((unit, Some(resolved)))
    })
    .await
}

async fn record<C, R, T, Resolving>(
    node: &Node,
    family: &Family,
    asked: &C,
    resolve: impl FnOnce() -> Resolving,
) -> Result<T, Failure>
where
    C: Serialize,
    R: Serialize,
    T: DeserializeOwned,
    Resolving: Future<Output = Result<(String, Option<R>), Failure>>,
{
    let told_apart =
        serde_json::to_vec(asked).map_err(|error| Failure::not_run(format!("recording {}: {error}", family.name)))?;
    let access = crate::record_access::current();
    let caller = fastctx_net::submission::current();
    fastctx_jobs::submitted::submit(
        node,
        family,
        &told_apart,
        caller.as_ref(),
        || async {
            let (unit, resolved) = resolve().await?;
            Ok((unit, Recording { access, asked, resolved }))
        },
        || {
            // The rest is the caller staying for the outcome, which takes
            // no place from the calls after it.
            fastctx_platform::call_timing::recorded();
            fastctx_net::link::admission::stays();
        },
    )
    .await
}

/// Takes back what a family was asked for `unit` and has not begun, as
/// when what it would have worked on is stopped: each such operation is
/// settled as not done, for `why`. Work that has begun goes on.
pub(crate) async fn withdraw(node: &Node, family: &Family, unit: &str, why: &str) -> Result<usize, Failure> {
    fastctx_jobs::submitted::withdraw(node, family, unit, why).await
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
    enlist(node, family, move |node, asked: C, _: Option<()>| doing(node, asked));
}

/// `register` for a family whose entrance is `submit_resolved`: `doing` is
/// given what was asked and what the entrance resolved.
#[expect(
    dead_code,
    reason = "the first family whose entrance resolves what its calls leave implicit is not merged yet"
)]
pub(crate) fn register_resolved<C, R, T, F, Doing>(node: &Arc<Node>, family: Family, doing: F)
where
    C: DeserializeOwned + Send + 'static,
    R: DeserializeOwned + Send + 'static,
    T: Serialize + Send + 'static,
    F: Fn(Arc<Node>, C, R) -> Doing + Send + Sync + 'static,
    Doing: Future<Output = Result<T, Failure>> + Send + 'static,
{
    let doing = Arc::new(doing);
    enlist(node, family, move |node, asked: C, resolved: Option<R>| {
        let doing = doing.clone();
        async move {
            match resolved {
                Some(resolved) => doing(node, asked, resolved).await,
                None => Err(Failure::not_run("what the call's entrance resolved was not recorded with it")),
            }
        }
    });
}

fn enlist<C, R, T, F, Doing>(node: &Arc<Node>, family: Family, doing: F)
where
    C: DeserializeOwned + Send + 'static,
    R: DeserializeOwned + Send + 'static,
    T: Serialize + Send + 'static,
    F: Fn(Arc<Node>, C, Option<R>) -> Doing + Send + Sync + 'static,
    Doing: Future<Output = Result<T, Failure>> + Send + 'static,
{
    fastctx_jobs::submitted::register(
        node,
        family,
        move |node: Arc<Node>, recorded: Recorded<C, R>, naming: Submission| {
            let Recorded { access, asked, resolved } = recorded;
            // It is accepted work: it waits its turn at a conversation's
            // gates, where a call would be told the conversation is busy.
            let working = crate::gate_waits::internal(doing(node, asked, resolved));
            let working = crate::record_access::within(access, working);
            // What the work asks of other machines is asked under this
            // operation's name, so doing it again asks them the same again.
            fastctx_net::submission::scope(Some(naming), working)
        },
    );
}
