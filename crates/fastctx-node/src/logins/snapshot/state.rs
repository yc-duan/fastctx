//! The derived login list, published atomically and read independently of refreshes.
use super::{build, legacy};
use crate::logins::kept;
use crate::{Node, passes};
use fastctx_hosts::inventory::walk;
use fastctx_platform::{Permissions, publish};
use fastctx_types::{
    ipc::Failure,
    logins::{AgentLogins, Identified, MachineLogins, Scan, Stage},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

const BYTES: u32 = 16 * 1024 * 1024;
const ROWS: usize = 4096;
const PASS: passes::Pass =
    passes::Pass { name: "logins/listing", within: Duration::from_secs(15), rests: Some(Duration::from_secs(60 * 60)) };
/// How old a projection a read is content with. Past it the read still
/// answers at once from what is held, and asks for a projection behind it:
/// what the user changed outside the World, in a login's own files or its
/// program, is not watched, and shows on the look after the one that asked.
const CONTENT: Duration = Duration::from_secs(30);
/// How long a read waits where no projection was ever made, as on the first
/// start of a version that keeps one: it answers what was read on the spot
/// before, not an empty list.
const FIRST: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Held {
    loaded: bool,
    value: Option<Arc<MachineLogins>>,
    agents: Option<Arc<fastctx_types::agents::AgentsReply>>,
    body: Vec<u8>,
    scan: Option<Option<Scan>>,
    identities: BTreeMap<(String, String), Identified>,
    changes: BTreeMap<(String, String), fastctx_types::logins::LoginChange>,
    quota: BTreeSet<(String, std::path::PathBuf)>,
    full: bool,
    cursor: usize,

    keys: Vec<String>,
    folders: Option<Vec<std::path::PathBuf>>,
    failed: Option<String>,
    /// Whether `value` is a projection, read from its file or made since;
    /// not what stands in for one before the first is made.
    projected: bool,
    /// When a whole projection was last made, or a read last asked for one.
    looked: Option<std::time::Instant>,
}

pub(crate) struct Listing {
    held: Mutex<Held>,
    making: Arc<tokio::sync::Mutex<()>>,
    /// Held by the read that has the first projection made, so that the
    /// reads beside it wait for that one.
    first: tokio::sync::Mutex<()>,
    readers: Arc<tokio::sync::Semaphore>,
    ready: AtomicBool,
}
impl Default for Listing {
    fn default() -> Self {
        Self {
            held: Mutex::new(Held::default()),
            making: Arc::new(tokio::sync::Mutex::new(())),
            first: tokio::sync::Mutex::new(()),
            readers: Arc::new(tokio::sync::Semaphore::new(2)),
            ready: AtomicBool::new(false),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    logins: MachineLogins,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<fastctx_types::agents::AgentsReply>,
}
fn path(node: &Node) -> std::path::PathBuf {
    node.world.path().join("agent-logins.json")
}
fn empty(node: &Node) -> MachineLogins {
    MachineLogins {
        machine: node.machine(),
        scan: None,
        codex: AgentLogins::default(),
        claude: AgentLogins::default(),
        by_login: true,
        others: BTreeMap::new(),
        folders: Vec::new(),
    }
}
fn validate(value: &MachineLogins) -> Result<(), Failure> {
    let rows = value.codex.listed.len()
        + value.codex.candidates.len()
        + value.claude.listed.len()
        + value.claude.candidates.len()
        + value.others.values().map(|side| side.listed.len() + side.candidates.len()).sum::<usize>();
    if rows > ROWS || value.folders.len() > 256 {
        return Err(Failure::new("the saved login list exceeds its row budget"));
    }
    Ok(())
}
fn load(node: &Node, held: &mut Held) -> Result<(), Failure> {
    if held.loaded {
        return Ok(());
    }
    held.loaded = true;
    let bytes = match fastctx_platform::durable_file::read_bounded(&path(node), BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // There is no connection-state value for "not read yet" in Login.
            // Before the first projection, show programs and scan progress only.
            let record = kept::read(node).map_err(|error| Failure::new(error.to_string()))?;
            let mut value = empty(node);
            value.codex.programs = record.codex.programs;
            value.claude.programs = record.claude.programs;
            value.scan = record.scan;
            held.value = Some(Arc::new(value));
            return Ok(());
        }
        Err(error) => return Err(Failure::new(format!("reading the saved login list: {error}"))),
    };
    let saved: Saved = serde_json::from_slice(&bytes)
        .map_err(|error| Failure::new(format!("reading the saved login list: {error}")))?;
    if saved.version != 1 {
        return Err(Failure::new("the saved login list uses an unknown format"));
    }
    validate(&saved.logins)?;
    if saved.agents.as_ref().is_some_and(|reply| reply.agents.len() > ROWS) {
        return Err(Failure::new("the saved older login reply exceeds its row budget"));
    }
    held.body = serde_json::to_vec(&saved).map_err(|error| Failure::new(error.to_string()))?;
    held.agents = saved.agents.map(Arc::new);
    held.value = Some(Arc::new(saved.logins));
    held.projected = true;
    Ok(())
}

/// What a read of the list does before it reads what is held. Where a
/// projection is held it does not wait: one older than a read is content
/// with is asked for behind it. Where none was ever made, or the one kept
/// cannot be read, it waits for one to be made.
pub(crate) async fn before_reading(node: &Arc<Node>) {
    restore(node).await;
    let (projected, old) = {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        let old = held.looked.is_none_or(|at| at.elapsed() >= CONTENT);
        if old {
            held.looked = Some(std::time::Instant::now());
        }
        (held.projected && held.failed.is_none(), old)
    };
    if !projected {
        first(node).await;
    } else if old {
        ask(node);
    }
}

/// Has the first projection made, once for all the reads waiting for it:
/// every row of it, within `FIRST`. A projection that cannot be made is
/// left to the passes; the read then says what is held, or why none is.
async fn first(node: &Arc<Node>) {
    let _one = node.logins.listing.first.lock().await;
    let until = std::time::Instant::now() + FIRST;
    let mut asked = false;
    loop {
        {
            let mut held = node.logins.listing.held.lock().expect("never poisoned");
            if held.projected && held.failed.is_none() && held.cursor == 0 {
                return;
            }
            if !asked {
                held.full = true;
                asked = true;
            }
        }
        if std::time::Instant::now() >= until || matches!(refresh(node).await, passes::Left::Work) {
            return;
        }
    }
}

/// Projects anew and waits for it, for an answer that shows what its call
/// just changed.
pub(crate) async fn renew(node: &Arc<Node>) {
    node.logins.listing.held.lock().expect("never poisoned").full = true;
    refresh(node).await;
}

/// Reads one bounded private file on first use; warm reads do no file IO.
pub(crate) fn read(node: &Node) -> Result<MachineLogins, Failure> {
    current(node).map(|(value, _)| value)
}

pub(crate) fn agents(node: &Node) -> Result<fastctx_types::agents::AgentsReply, Failure> {
    let (value, agents) = current(node)?;
    Ok(legacy::overlay(&value, agents.as_deref()))
}

type Current = (MachineLogins, Option<Arc<fastctx_types::agents::AgentsReply>>);
fn current(node: &Node) -> Result<Current, Failure> {
    let (value, scan, identities, folders, changes, agents) = {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        if let Err(error) = load(node, &mut held) {
            held.failed = Some(error.message);
        }
        if let Some(error) = &held.failed {
            return Err(Failure::new(error.clone()));
        }
        (
            held.value.clone().expect("loaded snapshot"),
            held.scan.clone(),
            held.identities.clone(),
            held.folders.clone(),
            held.changes.clone(),
            held.agents.clone(),
        )
    };
    let mut value = (*value).clone();
    value.machine = node.machine();
    if let Some(scan) = scan {
        value.scan = scan;
    }
    if let Some(scan) = &mut value.scan
        && scan.usual == Stage::Running
        && !node.logins.scanning.load(Ordering::Acquire)
    {
        scan.usual = Stage::Failed { at: jiff::Timestamp::now(), reason: "the node stopped during it".into() };
    }

    for (agent, side) in [("codex", &mut value.codex), ("claude", &mut value.claude)]
        .into_iter()
        .chain(value.others.iter_mut().map(|(agent, side)| (agent.as_str(), side)))
    {
        for login in side.listed.iter_mut().chain(&mut side.candidates) {
            let key = (agent.to_string(), walk::key(&login.dir));
            login.change = changes.get(&key).cloned();
            if let Some(identity) = identities.get(&key) {
                if login.identity.as_ref().map(|known| &known.who) != Some(&identity.who) {
                    login.limits = fastctx_types::agents::Fact::unavailable(
                        "the login changed; its current usage has not been projected yet",
                    );
                }
                login.identity = Some(identity.clone());
            }
        }
    }
    if let Some(folders) = folders {
        let mut prior: BTreeMap<_, _> =
            value.folders.into_iter().map(|folder| (walk::key(&folder.dir), folder.search)).collect();
        value.folders = folders
            .into_iter()
            .map(|dir| fastctx_types::logins::Folder { search: prior.remove(&walk::key(&dir)).flatten(), dir })
            .collect();
    }
    crate::logins::search_progress::overlay(node, &mut value);
    Ok((value, agents))
}

pub(crate) fn register(node: &Arc<Node>) {
    node.logins.listing.ready.store(true, Ordering::Release);
    passes::register(node, PASS, |node| async move { refresh(&node).await });
}
pub(crate) fn ask(node: &Node) {
    {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        held.full = true;
    }
    if node.logins.listing.ready.load(Ordering::Acquire) {
        passes::ask(node, PASS.name);
    }
}
pub(crate) fn progress(node: &Node, agent: &str, dir: &Path, change: fastctx_types::logins::LoginChange) {
    let value = node.logins.listing.held.lock().expect("never poisoned").value.clone();
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(side) = value.as_ref().and_then(|value| value.of(agent)) {
        dirs.extend(
            side.listed
                .iter()
                .chain(&side.candidates)
                .filter(|login| crate::homes::same_dir(&login.dir, dir))
                .map(|login| login.dir.clone()),
        );
    }
    let mut held = node.logins.listing.held.lock().expect("never poisoned");
    for dir in dirs {
        let key = (agent.to_string(), walk::key(&dir));
        if !held.changes.contains_key(&key) && held.changes.len() >= ROWS {
            held.changes.pop_first();
        }
        held.changes.insert(key, change.clone());
    }
}
pub(crate) fn chosen(node: &Node, added: &kept::Added) {
    node.logins.listing.held.lock().expect("never poisoned").folders = Some(added.folders.clone());
    ask(node);
}
pub(crate) fn quota(node: &Node, agent: &str, dir: &Path) {
    // Usage is read with every step of a turn: a pass is asked for by the
    // first login waiting for one, and takes all that wait by then.
    let first = {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        let first = held.quota.is_empty();
        held.quota.insert((agent.to_string(), dir.to_path_buf()));
        first
    };
    if first && node.logins.listing.ready.load(Ordering::Acquire) {
        passes::ask(node, PASS.name);
    }
}

/// Publishes cheap progress and identities before telling readers they changed.
pub(crate) fn kept(node: &Node, before: &kept::Kept, after: &kept::Kept) {
    {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        held.scan = Some(after.scan.clone());
        held.identities.clear();
        for (agent, record) in [("codex", &after.codex), ("claude", &after.claude)]
            .into_iter()
            .chain(after.others.iter().map(|(agent, record)| (agent.as_str(), record)))
        {
            held.identities.extend(
                record
                    .logins
                    .iter()
                    .map(|login| ((agent.to_string(), walk::key(&login.dir)), login.identified.clone())),
            );
        }
    }
    let mut before = before.clone();
    let mut after = after.clone();
    before.codex.limits.clear();
    after.codex.limits.clear();
    before.scan = None;
    after.scan = None;
    if serde_json::to_vec(&before).ok() != serde_json::to_vec(&after).ok() {
        ask(node);
    }
}

/// A timed-out blocking read keeps its permit until it actually finishes.
pub(crate) async fn part<T: Send + 'static>(
    node: &Arc<Node>,
    what: &'static str,
    within: Duration,
    work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let permit = node
        .logins
        .listing
        .readers
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure::new("the login refresh has two reads still in progress"))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let checking = cancelled.clone();
    let task = fastctx_platform::task::spawn_blocking(move || {
        let _permit = permit;
        if checking.load(Ordering::Acquire) {
            return Err(Failure::not_run("the login read expired before it began"));
        }
        fastctx_platform::read_budget::within(crate::logins::read_limits::PROJECTION, work)
            .map_err(|error| Failure::new(error.to_string()))?
    });
    match tokio::time::timeout(within, task).await {
        Ok(result) => result.map_err(|error| Failure::new(error.to_string()))?,
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            Err(Failure::new(format!("{what} exceeded the login refresh time budget")))
        }
    }
}

async fn restore(node: &Arc<Node>) {
    if node.logins.listing.held.lock().expect("never poisoned").value.is_some() {
        return;
    }
    let reading = node.clone();
    let restored = part(node, "restoring the login projection", Duration::from_secs(2), move || {
        // This is a local holder: no reader's lock covers the disk operation.
        let mut restored = Held::default();
        load(&reading, &mut restored)?;
        Ok((restored.value, restored.agents, restored.body, restored.projected))
    })
    .await;
    match restored {
        Ok((value, agents, body, projected)) => {
            let mut held = node.logins.listing.held.lock().expect("never poisoned");
            if held.value.is_none() {
                held.loaded = true;
                held.failed = None;
                held.value = value;
                held.agents = agents;
                held.body = body;
                held.projected = projected;
            }
        }
        Err(error) => node.log.line(format_args!("restoring the login list: {}", error.message)),
    }
}

pub(crate) async fn refresh(node: &Arc<Node>) -> passes::Left {
    let making = node.logins.listing.making.clone().lock_owned().await;
    restore(node).await;
    let (full, quota, prior, cursor, keys, prior_agents) = {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        let quota = std::mem::take(&mut held.quota);
        // A request arriving during a cycle also needs the already-read head
        // refreshed, but must not keep resetting the tail's progress.
        let full = if held.cursor != 0 { held.full } else { std::mem::take(&mut held.full) };
        (full, quota, held.value.clone(), held.cursor, held.keys.clone(), held.agents.clone())
    };
    let refresh_programs = full || cursor != 0 || quota.is_empty() || prior.is_none();
    let built = if let Some(prior) = prior.as_ref().filter(|_| !refresh_programs) {
        build::quotas(node, (**prior).clone(), quota).await.map(|value| build::Built {
            value,
            next: cursor,
            more: full || cursor != 0,
            keys,
        })
    } else {
        if !quota.is_empty() {
            node.logins.listing.held.lock().expect("never poisoned").quota.extend(quota);
        }
        build::all(node, prior.as_deref(), cursor, &keys).await
    };
    match built {
        Ok(build::Built { value, next, more, keys }) => {
            let agents = if refresh_programs {
                legacy::project(node, &value).await
            } else {
                legacy::overlay(&value, prior_agents.as_deref())
            };
            let saving = node.clone();
            match fastctx_platform::task::spawn_blocking(move || {
                let _making = making;
                save(&saving, value, agents)
            })
            .await
            {
                Ok(Ok(())) => {
                    let again = {
                        let mut held = node.logins.listing.held.lock().expect("never poisoned");
                        held.cursor = next;
                        held.keys = keys;
                        held.projected = true;
                        if refresh_programs && !more {
                            held.looked = Some(std::time::Instant::now());
                        }
                        more || held.full || !held.quota.is_empty()
                    };
                    if again && node.logins.listing.ready.load(Ordering::Acquire) {
                        passes::ask(node, PASS.name);
                    }
                    passes::Left::Nothing
                }
                Ok(Err(error)) => {
                    node.log.line(format_args!("keeping the login list: {}", error.message));
                    passes::Left::Work
                }
                Err(error) => {
                    node.log.line(format_args!("keeping the login list: {error}"));
                    passes::Left::Work
                }
            }
        }
        Err(error) => {
            node.log.line(format_args!("assembling the login list: {}", error.message));
            passes::Left::Work
        }
    }
}
fn save(node: &Node, mut value: MachineLogins, agents: fastctx_types::agents::AgentsReply) -> Result<(), Failure> {
    validate(&value)?;
    for side in [&mut value.codex, &mut value.claude].into_iter().chain(value.others.values_mut()) {
        for login in side.listed.iter_mut().chain(&mut side.candidates) {
            login.change = None;
        }
    }
    let body = serde_json::to_vec(&Saved { version: 1, logins: value.clone(), agents: Some(agents.clone()) })
        .map_err(|error| Failure::new(error.to_string()))?;
    let same = node.logins.listing.held.lock().expect("never poisoned").body == body;
    if same {
        return Ok(());
    }
    let bytes = &body;
    if bytes.len() > BYTES as usize {
        return Err(Failure::new("the derived login list exceeds 16 MiB"));
    }
    publish(&path(node), bytes, Permissions::Private).map_err(|error| Failure::new(error.to_string()))?;
    {
        let mut held = node.logins.listing.held.lock().expect("never poisoned");
        held.loaded = true;
        held.failed = None;
        held.body = body;
        held.value = Some(Arc::new(value));
        held.agents = Some(Arc::new(agents));
    }
    node.topics.changed(fastctx_types::topics::LOGINS);
    Ok(())
}
