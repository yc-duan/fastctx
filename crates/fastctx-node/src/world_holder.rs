//! Taking the World from a node that holds it while no service keeps it.
//!
//! A node the service keeps waits for the World's lock. The node holding it
//! may be one nobody keeps: a renewal's bridge whose coordinator gave up and
//! left it serving, or one started by hand. Such a holder is asked to hand
//! the World over once idle (`handoff::hand_over`), unless a renewal is still
//! under way, which hands the World on itself. A holder whose endpoint takes
//! connections but answers nothing for `UNANSWERED` is ended: it keeps the
//! World and serves nobody.

use std::path::Path;
use std::time::{Duration, Instant};

use fastctx_client::renewal::Probe;
use fastctx_platform::FileLock;
use fastctx_types::ProcessIdentity;
use fastctx_types::layout::WorldDir;
use fastctx_types::renewal::{Action, Phase};

/// How long this node waits before it first looks at the holder, and then
/// between looks.
const FIRST_LOOK: Duration = Duration::from_secs(5);
const LOOK_EVERY: Duration = Duration::from_secs(30);
/// How long a look waits for the holder to answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// How long a holder takes connections and answers nothing before it is
/// ended. Longer than any startup of a node between listening and answering.
const UNANSWERED: Duration = Duration::from_secs(10 * 60);
/// How often the lock is tried while waiting has a deadline.
const TRY_EVERY: Duration = Duration::from_millis(250);

/// Takes the World's lock for the node at `root`, waiting for it until
/// `deadline` where there is one; `None` once that passed while the holder
/// answers. One that answers nothing is waited for past it, to be ended.
pub(crate) async fn take(root: &Path, world: &WorldDir, deadline: Option<Instant>) -> anyhow::Result<Option<FileLock>> {
    let lock = world.node_lock();
    let mut looking = Looking::default();
    let mut next_look = Instant::now() + FIRST_LOOK;
    match deadline {
        // Woken the moment the lock is let go.
        None => {
            let taking = lock.clone();
            let mut acquired = fastctx_platform::task::spawn_blocking(move || FileLock::acquire(&taking));
            loop {
                let wait = next_look.saturating_duration_since(Instant::now());
                tokio::select! {
                    taken = &mut acquired => return Ok(Some(taken??)),
                    () = tokio::time::sleep(wait) => {
                        looking.look(root, world).await;
                        next_look = Instant::now() + LOOK_EVERY;
                    }
                }
            }
        }
        Some(deadline) => loop {
            let trying = lock.clone();
            if let Some(taken) = fastctx_platform::task::blocking(move || FileLock::try_acquire(&trying)).await? {
                return Ok(Some(taken));
            }
            let now = Instant::now();
            if now >= deadline && looking.unanswered_since.is_none() {
                return Ok(None);
            }
            if now >= next_look {
                looking.look(root, world).await;
                next_look = Instant::now() + LOOK_EVERY;
            }
            tokio::time::sleep(TRY_EVERY).await;
        },
    }
}

#[derive(Default)]
struct Looking {
    /// The holder asked to hand the World over, asked once.
    asked: Option<ProcessIdentity>,
    /// Since when the holder's endpoint takes connections and answers
    /// nothing.
    unanswered_since: Option<Instant>,
}

impl Looking {
    async fn look(&mut self, root: &Path, world: &WorldDir) {
        if renewal_under_way(root).await {
            self.unanswered_since = None;
            return;
        }
        match tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::probe(world)).await {
            Ok(Ok(Probe::Running(node))) => {
                self.unanswered_since = None;
                if node.supervised || node.yielding || self.asked.as_ref() == Some(&node.process) {
                    return;
                }
                let kind = if node.bridge { "a renewal's bridge" } else { "a node no service keeps" };
                self.ask(world, &node.process, kind).await;
            }
            // An earlier release's node, asked in its own protocol.
            Ok(Ok(Probe::Legacy { .. })) => {
                self.unanswered_since = None;
                if self.asked.is_none()
                    && let Ok(Ok(Some(process))) =
                        tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::process(world)).await
                {
                    self.ask(world, &process, "an earlier release's node").await;
                }
            }
            // Nobody listens: the holder is starting or ending, which
            // nobody can tell apart from here; it is left to finish.
            Ok(Ok(Probe::Absent)) => self.unanswered_since = None,
            Ok(Err(_)) | Err(_) => {
                let since = *self.unanswered_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= UNANSWERED {
                    self.unanswered_since = None;
                    end_silent(world).await;
                }
            }
        }
    }

    async fn ask(&mut self, world: &WorldDir, holder: &ProcessIdentity, kind: &str) {
        match tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::request(world, Action::Yield)).await {
            Ok(Ok(())) => {
                eprintln!(
                    "fastctx node: asked process {} holding the World, {kind}, to hand it over once idle",
                    holder.pid
                );
                self.asked = Some(holder.clone());
            }
            Ok(Err(error)) => eprintln!("fastctx node: asking process {} to hand the World over: {error}", holder.pid),
            Err(_) => eprintln!("fastctx node: process {} did not answer when asked to hand the World over", holder.pid),
        }
    }
}

/// Ends the node serving the World's endpoint, which takes connections and
/// has answered nothing for `UNANSWERED`.
async fn end_silent(world: &WorldDir) {
    let Ok(Ok(Some(holder))) = tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::process(world)).await
    else {
        eprintln!("fastctx node: the node holding the World answers nothing, and its process cannot be told");
        return;
    };
    if fastctx_platform::current_identity().is_ok_and(|me| me == holder) {
        return;
    }
    match fastctx_platform::task::blocking(move || fastctx_platform::terminate::terminate(&holder).map(|()| holder)).await {
        Ok(holder) => eprintln!(
            "fastctx node: ended process {}, which held the World and answered nothing for {} minutes",
            holder.pid,
            UNANSWERED.as_secs() / 60
        ),
        Err(error) => eprintln!("fastctx node: ending the node that holds the World and answers nothing: {error}"),
    }
}

/// Whether a renewal is handing the World on now: its plan is not over, and
/// the coordinator that keeps its bridge lives.
async fn renewal_under_way(root: &Path) -> bool {
    let root = root.to_path_buf();
    fastctx_platform::task::blocking(move || -> std::io::Result<bool> {
        let Some(plan) = fastctx_install::renewal::read(&root).map_err(std::io::Error::other)? else {
            return Ok(false);
        };
        if matches!(plan.phase, Phase::Complete | Phase::Failed) {
            return Ok(false);
        }
        let witness = fastctx_install::renewal::coordinator(&root, &plan.candidate.build.digest);
        let Some(coordinator) =
            fastctx_install::renewal::read_json::<ProcessIdentity>(&witness).map_err(std::io::Error::other)?
        else {
            return Ok(false);
        };
        fastctx_platform::is_alive(&coordinator)
    })
    .await
    // Unsure: a renewal may be under way.
    .unwrap_or(true)
}
