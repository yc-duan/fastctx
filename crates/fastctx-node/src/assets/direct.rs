//! A conversation's streams kept by the machine that orders them
//! (`fastctx_types::assets::DIRECT`, `fastctx_types::asset_peers`).
//!
//! With a hub that speaks it, what this machine orders of a conversation
//! stays here: no frame or object of it goes to the hub, which is told only
//! that a stream began (`Request::Begin`, so it signs the stream's first
//! epoch) and where it stands (`Request::Stirred`, so the machines that hold
//! the scope are told it moved). A machine that holds a copy takes the
//! frames and objects from the machine that orders them, over a data
//! connection a call owns: directly where the two are linked, through the
//! hub where they are not. Every ask carries the hub's word that the asking
//! device holds the scope (`Request::Permits`): who may read what is judged
//! on the hub and nowhere else.
//!
//! Nothing here cuts, sets aside or sends back what a copy holds because
//! the hub holds less of it: the hub holding less is how it is meant to be.

use super::Assets;
use super::session::{ask, blocking};
use fastctx_net::data::Stream;
use fastctx_net::link::{Feed, Link};
use fastctx_net::sign::PublicKey;
use fastctx_types::access::{HubId, Purpose};
use fastctx_types::asset_lines::{FRAME_BYTES, RUN_BYTES};
use fastctx_types::asset_peers::{Ask, Head, Permit, SCOPES, Told};
use fastctx_types::assets::{self as wire, APART_FRAME_BYTES, DIRECT, Reply, Request};
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// How long a line to a machine nothing is asked on is kept open.
const KEPT_FOR: Duration = Duration::from_secs(45);
/// How long one exchange on a line may take, the line's opening included.
const EXCHANGED_WITHIN: Duration = Duration::from_secs(120);
/// How long a machine that gave nothing when asked is left alone.
const LEFT_ALONE: Duration = Duration::from_secs(60);
/// How long a stream whose frames took it nowhere is not asked for again
/// from the same place.
const STALLED_FOR: Duration = Duration::from_secs(60);
/// How long before the hub's word runs out another is asked for, in
/// milliseconds.
const RENEWED_BEFORE: i64 = 30_000;
/// How far past its end the hub's word still stands here, the two clocks
/// differing, in milliseconds.
pub(super) const CLOCKS_APART: i64 = 30_000;
/// How often the machines that order what this one holds are asked where
/// their streams stand: what catches a word of moving lost on the way.
const LOOKED_EVERY: Duration = Duration::from_secs(600);
/// As many machines are given what is kept here at once.
const GIVEN_AT_ONCE: usize = 4;
/// What a machine that gives as much as it gives at once answers.
const BUSY: &str = "it gives as much as it gives at once";
/// What begins the failure of taking something from the machine that keeps
/// it for a reason of that machine's, or of the way to it: it did not
/// answer, refused, or the hub gave no word for it. What it holds may be
/// taken later; nothing is wrong with what is here.
pub(super) const UNREACHED: &str = "not taken from its machine now: ";
/// What a machine's refusal of one ask begins: the machine itself answers.
const REFUSED: &str = "it refused: ";

/// What this machine keeps of taking from, and giving to, other machines.
pub(crate) struct Direct {
    /// The hub's word that this machine holds a scope, by hub and scope.
    permits: Mutex<HashMap<(HubId, String), Arc<Permit>>>,
    /// The machines that gave nothing when asked, and since when.
    left_alone: Mutex<HashMap<(HubId, MachineName), Instant>>,
    /// The lines open to machines that nothing is asked on now.
    lines: Mutex<HashMap<(HubId, MachineName), Vec<Line>>>,
    /// How far each hub's clock is ahead of this machine's, in milliseconds.
    clocks: Mutex<HashMap<HubId, i64>>,
    /// The streams kept here the hub signed a first epoch of.
    begun: Mutex<HashSet<(HubId, String, String)>>,
    /// Where each stream kept here stood as the hub was last told.
    told: Mutex<HashMap<(HubId, String, String), u64>>,
    /// The streams whose frames took them nowhere, from where and since when.
    stalled: Mutex<HashMap<(String, String), (u64, Instant)>>,
    /// When the machines of each hub were last asked where their streams
    /// stand.
    looked: Mutex<HashMap<HubId, Instant>>,
    /// The room for machines being given what is kept here.
    pub(super) giving: Arc<tokio::sync::Semaphore>,
}

impl Default for Direct {
    fn default() -> Self {
        Self {
            permits: Mutex::default(),
            left_alone: Mutex::default(),
            lines: Mutex::default(),
            clocks: Mutex::default(),
            begun: Mutex::default(),
            told: Mutex::default(),
            stalled: Mutex::default(),
            looked: Mutex::default(),
            giving: Arc::new(tokio::sync::Semaphore::new(GIVEN_AT_ONCE)),
        }
    }
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

impl Direct {
    /// The hub said what its clock shows, as a session opened.
    pub(super) fn hub_clock(&self, hub: &HubId, clock: i64) {
        self.clocks.lock().expect("clocks").insert(hub.clone(), clock.saturating_sub(now_ms()));
    }

    /// What the hub's clock shows now, as far as this machine knows.
    pub(super) fn hub_now(&self, hub: &HubId) -> i64 {
        now_ms().saturating_add(self.clocks.lock().expect("clocks").get(hub).copied().unwrap_or(0))
    }

    fn is_left_alone(&self, hub: &HubId, machine: &MachineName) -> bool {
        let mut left = self.left_alone.lock().expect("left alone");
        left.retain(|_, since| since.elapsed() < LEFT_ALONE);
        left.contains_key(&(hub.clone(), machine.clone()))
    }

    fn leave_alone(&self, hub: &HubId, machine: &MachineName) {
        self.left_alone.lock().expect("left alone").insert((hub.clone(), machine.clone()), Instant::now());
    }

    /// Whether `stream` of `scope` took nothing from `at` lately: it is not
    /// asked for again from there until a while passed.
    pub(super) fn stalled(&self, scope: &str, stream: &str, at: u64) -> bool {
        let mut stalled = self.stalled.lock().expect("stalled");
        stalled.retain(|_, (_, since)| since.elapsed() < STALLED_FOR);
        stalled.get(&(scope.to_string(), stream.to_string())).is_some_and(|(from, _)| *from == at)
    }

    /// `stream` of `scope` took nothing from `at`.
    pub(super) fn stall(&self, scope: &str, stream: &str, at: u64) {
        self.stalled.lock().expect("stalled").insert((scope.to_string(), stream.to_string()), (at, Instant::now()));
    }

    /// Whether the machines of `hub` are to be asked where their streams
    /// stand now: not asked for a while, or never; noted as asked.
    pub(super) fn look_due(&self, hub: &HubId) -> bool {
        let mut looked = self.looked.lock().expect("looked");
        if looked.get(hub).is_some_and(|at| at.elapsed() < LOOKED_EVERY) {
            return false;
        }
        looked.insert(hub.clone(), Instant::now());
        true
    }
}

/// Whether, with this hub, what a machine orders of `scope` stays on that
/// machine.
pub(crate) fn kept_by_machines(link: &Link, scope: &str) -> bool {
    link.asset_version() >= DIRECT && wire::of_conversation(scope)
}

/// Whether `stream` of a conversation's scope is one a machine orders: the
/// hub orders the rest itself, and keeps them.
pub(crate) fn machine_orders(stream: &str) -> bool {
    !fastctx_content::v2::hub_orders(stream)
}

/// Whether `stream` of `scope` stays on the machine that orders it.
pub(crate) fn stays(link: &Link, scope: &str, stream: &str) -> bool {
    kept_by_machines(link, scope) && machine_orders(stream)
}

/// The machine that orders `stream` of `scope` as this machine's library
/// knows, where that is another machine of the hub's roster.
async fn orderer(assets: &Assets, link: &Link, scope: &str, stream: &str) -> Result<Option<MachineName>, String> {
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let Some(device) = blocking(&assets.library, move |library| library.orderer(&scope_text, &stream_text)).await? else {
        return Ok(None);
    };
    if device == assets.device() {
        return Ok(None);
    }
    Ok(link.view().by_key(&PublicKey::from_bytes(device)).map(|member| member.name.clone()))
}

/// The machine `stream` of `scope` is taken from: the one that orders it,
/// where the stream stays there and that machine is not left alone now.
pub(super) async fn source(assets: &Assets, hub: &HubId, link: &Link, scope: &str, stream: &str) -> Option<MachineName> {
    if !stays(link, scope, stream) {
        return None;
    }
    let machine = orderer(assets, link, scope, stream).await.ok().flatten()?;
    (!assets.direct.is_left_alone(hub, &machine)).then_some(machine)
}

/// An account logged in here that may ask `to` of the conversations it
/// reads there.
fn speaker(link: &Link, to: &MachineName) -> Option<AccountName> {
    let view = link.view();
    let me = view.me.clone();
    let logins: Vec<AccountName> = view.me().map(|member| member.logins.iter().cloned().collect()).unwrap_or_default();
    logins.into_iter().find(|account| view.admits(&me, account, to, Purpose::Agents).is_ok())
}

/// Asks the hub's word for `scopes`, as many as one word names at most, and
/// keeps it for each scope it grants.
async fn permits(assets: &Assets, hub: &HubId, link: &Arc<Link>, scopes: Vec<String>) -> Result<Arc<Permit>, String> {
    let Reply::Permits { until, granted, word } = ask(assets, link, Request::Permits { scopes }).await? else {
        return Err("the hub did not say what this machine holds".into());
    };
    let permit = Arc::new(Permit { until, granted, word });
    let mut kept = assets.direct.permits.lock().expect("permits");
    let now = assets.direct.hub_now(hub);
    kept.retain(|_, permit| permit.until > now);
    for scope in &permit.granted {
        kept.insert((hub.clone(), scope.clone()), permit.clone());
    }
    Ok(permit)
}

/// The hub's word that this machine holds `scope`, one kept that stands a
/// while yet or one asked for now.
async fn permit(assets: &Assets, hub: &HubId, link: &Arc<Link>, scope: &str) -> Result<Arc<Permit>, String> {
    let kept = assets.direct.permits.lock().expect("permits").get(&(hub.clone(), scope.to_string())).cloned();
    if let Some(kept) = kept
        && kept.until.saturating_sub(assets.direct.hub_now(hub)) > RENEWED_BEFORE
    {
        return Ok(kept);
    }
    let permit = permits(assets, hub, link, vec![scope.to_string()]).await?;
    if permit.granted.iter().any(|granted| granted == scope) {
        Ok(permit)
    } else {
        Err(format!("the hub does not say this machine holds {scope}"))
    }
}

/// A line to a machine: the data connection of a call it answers
/// (`RemoteCall::Assets`), one exchange on it at a time.
struct Line {
    jobs: mpsc::Sender<Job>,
}

/// One exchange asked on a line, and where its answer goes.
enum Job {
    Heads { ask: Ask, answer: oneshot::Sender<Result<Vec<Head>, String>> },
    Frames { ask: Ask, answer: oneshot::Sender<Result<Vec<wire::Frame>, String>> },
    Objects { ask: Ask, wanted: Vec<wire::ObjectRef>, answer: oneshot::Sender<Result<Vec<Option<Vec<u8>>>, String>> },
}

/// What a machine said where an answer of another kind was due.
fn refused(told: Told) -> String {
    match told {
        Told::Failed { why } => format!("{REFUSED}{why}"),
        Told::Busy => BUSY.into(),
        _ => "it answered with something else".into(),
    }
}

async fn heads_on(stream: &mut Stream, ask: &Ask) -> Result<Vec<Head>, String> {
    stream.tell(ask).await?;
    match stream.told::<Told>("where a machine's streams stand").await? {
        Told::Heads { heads } => Ok(heads),
        other => Err(refused(other)),
    }
}

async fn frames_on(stream: &mut Stream, ask: &Ask) -> Result<Vec<wire::Frame>, String> {
    stream.tell(ask).await?;
    let mut frames = match stream.told::<Told>("a machine's frames").await? {
        Told::Frames { frames } => frames,
        other => return Err(refused(other)),
    };
    let bodies = stream.given(FRAME_BYTES as u64 + APART_FRAME_BYTES, "the frames' bodies").await?;
    let mut rest = bodies.as_slice();
    for frame in &mut frames {
        let length = frame.apart as usize;
        if !frame.body.0.is_empty() || length > rest.len() {
            return Err("the frames' bodies are not as long as the frames said".into());
        }
        let (body, after) = rest.split_at(length);
        frame.body = body.to_vec().into();
        frame.apart = 0;
        rest = after;
    }
    if !rest.is_empty() {
        return Err("more bytes of bodies came than the frames said".into());
    }
    Ok(frames)
}

async fn objects_on(stream: &mut Stream, ask: &Ask, wanted: &[wire::ObjectRef]) -> Result<Vec<Option<Vec<u8>>>, String> {
    stream.tell(ask).await?;
    let mut given = Vec::with_capacity(wanted.len());
    for (index, object) in wanted.iter().enumerate() {
        match stream.told::<Told>("a machine's word on an object").await? {
            Told::Object { index: said, given: true, .. } if said as usize == index => {
                let mut bytes = Vec::with_capacity(object.size as usize);
                while (bytes.len() as u64) < object.size {
                    let run = stream.given(RUN_BYTES.min(object.size - bytes.len() as u64), "an object's bytes").await?;
                    if run.is_empty() {
                        return Err("an object's bytes ended before its length".into());
                    }
                    bytes.extend_from_slice(&run);
                }
                given.push(Some(bytes));
            }
            Told::Object { index: said, given: false, .. } if said as usize == index => given.push(None),
            other if index == 0 => return Err(refused(other)),
            _ => return Err("its words on the objects do not come one for each, in order".into()),
        }
    }
    Ok(given)
}

/// Carries a line: each exchange asked on it in turn, until one fails or
/// nothing is asked for a while. Dropping the connection ends the call.
async fn carry(mut stream: Stream, mut jobs: mpsc::Receiver<Job>) -> Result<(), String> {
    while let Ok(Some(job)) = tokio::time::timeout(KEPT_FOR, jobs.recv()).await {
        let failed = match job {
            Job::Heads { ask, answer } => {
                let done = heads_on(&mut stream, &ask).await;
                let failed = done.is_err();
                let _ = answer.send(done);
                failed
            }
            Job::Frames { ask, answer } => {
                let done = frames_on(&mut stream, &ask).await;
                let failed = done.is_err();
                let _ = answer.send(done);
                failed
            }
            Job::Objects { ask, wanted, answer } => {
                let done = objects_on(&mut stream, &ask, &wanted).await;
                let failed = done.is_err();
                let _ = answer.send(done);
                failed
            }
        };
        // What was said and sent part way cannot be told from the next
        // exchange: the line ends.
        if failed {
            break;
        }
    }
    Ok(())
}

/// A line to `to`: one open that nothing is asked on, or a new one.
fn line(assets: &Assets, hub: &HubId, link: &Arc<Link>, to: &MachineName) -> Result<Line, String> {
    let kept = assets.direct.lines.lock().expect("lines").get_mut(&(hub.clone(), to.clone())).and_then(|lines| {
        lines.retain(|line| !line.jobs.is_closed());
        lines.pop()
    });
    if let Some(kept) = kept {
        return Ok(kept);
    }
    let account = speaker(link, to).ok_or_else(|| format!("no account logged in here may ask {to} of its conversations"))?;
    let (jobs, taken) = mpsc::channel::<Job>(1);
    let feed: Feed = Box::new(move |stream| Box::pin(carry(stream, taken)));
    let (link, to) = (link.clone(), to.clone());
    fastctx_platform::task::spawn(async move {
        let _ = link.call_with_stream(&to, &account, Purpose::Agents, RemoteCall::Assets, feed).await;
    });
    Ok(Line { jobs })
}

/// Asks one exchange of `to` on a line and waits for its answer; the line
/// is kept for the next where it went well, and the machine left alone a
/// while where it did not.
async fn exchange<T>(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    to: &MachineName,
    job: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Job,
) -> Result<T, String> {
    if assets.direct.is_left_alone(hub, to) {
        return Err(format!("{UNREACHED}{to} gave nothing lately and is left alone for now"));
    }
    let line = line(assets, hub, link, to).map_err(|why| format!("{UNREACHED}{why}"))?;
    let (answer, answered) = oneshot::channel();
    let asked = async {
        line.jobs.send(job(answer)).await.map_err(|_| format!("the line to {to} ended"))?;
        answered.await.map_err(|_| format!("{to} did not take the line"))?
    };
    let done = match tokio::time::timeout(EXCHANGED_WITHIN, asked).await {
        Ok(done) => done,
        Err(_) => Err(format!("{to} did not answer within {} s", EXCHANGED_WITHIN.as_secs())),
    };
    match &done {
        Ok(_) => assets.direct.lines.lock().expect("lines").entry((hub.clone(), to.clone())).or_default().push(line),
        // One that refused this ask answers others: only one that did not
        // answer, or is busy, is left alone a while.
        Err(why) if why.starts_with(REFUSED) => {}
        Err(_) => assets.direct.leave_alone(hub, to),
    }
    done.map_err(|why| format!("{UNREACHED}{to}: {why}"))
}

/// Where the streams `machine` orders of `scopes` stand, of those the hub
/// says this machine holds.
pub(super) async fn heads(assets: &Assets, hub: &HubId, link: &Arc<Link>, machine: &MachineName, scopes: Vec<String>) -> Result<Vec<Head>, String> {
    let mut heads = Vec::new();
    for some in scopes.chunks(SCOPES) {
        // One word for one scope is kept and used again; for several, asked.
        let permit = match some {
            [scope] => permit(assets, hub, link, scope).await?,
            _ => permits(assets, hub, link, some.to_vec()).await?,
        };
        if permit.granted.is_empty() {
            continue;
        }
        let ask = Ask::Heads { permit: (*permit).clone(), scopes: permit.granted.clone() };
        heads.extend(exchange(assets, hub, link, machine, |answer| Job::Heads { ask, answer }).await?);
    }
    Ok(heads)
}

/// Up to a line's worth of frames of `stream` of `scope` from the one
/// holding `from`, each whole, taken from `machine`.
pub(super) async fn frames(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    machine: &MachineName,
    scope: &str,
    stream: &str,
    from: u64,
) -> Result<Vec<wire::Frame>, String> {
    let permit = permit(assets, hub, link, scope).await.map_err(|why| format!("{UNREACHED}{why}"))?;
    let ask = Ask::Frames {
        permit: (*permit).clone(),
        scope: scope.to_string(),
        stream: stream.to_string(),
        from,
        frames: fastctx_types::asset_lines::FRAMES as u32,
    };
    exchange(assets, hub, link, machine, |answer| Job::Frames { ask, answer }).await
}

/// `objects` of `scope` taken from `machine`: the bytes of each it gave.
pub(super) async fn objects(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    machine: &MachineName,
    scope: &str,
    objects: Vec<wire::ObjectRef>,
) -> Result<Vec<Option<Vec<u8>>>, String> {
    let permit = permit(assets, hub, link, scope).await?;
    let ask = Ask::Objects { permit: (*permit).clone(), scope: scope.to_string(), objects: objects.clone() };
    exchange(assets, hub, link, machine, |answer| Job::Objects { ask, wanted: objects, answer }).await
}

/// The machine an object `scope`'s frames refer to is taken from: the one
/// that orders a stream it is read through here, where that stream stays
/// on its machine.
pub(super) async fn object_source(assets: &Assets, hub: &HubId, link: &Link, scope: &str, hash: [u8; 32]) -> Option<MachineName> {
    if !kept_by_machines(link, scope) {
        return None;
    }
    let scope_text = scope.to_string();
    let read = blocking(&assets.library, move |library| library.streams_reading(&scope_text, &hash)).await.ok()?;
    for stream in read {
        if let Some(machine) = source(assets, hub, link, scope, &stream).await {
            return Some(machine);
        }
    }
    None
}

/// Tells the hub of the streams of `scope` this machine orders and keeps:
/// each begun there once, and where it stands whenever that moved. What
/// could not be told is told as the scope is next gone through; nothing of
/// the streams themselves depends on it.
pub(super) async fn announce(assets: &Assets, hub: &HubId, link: &Arc<Link>, scope: &str) {
    if !kept_by_machines(link, scope) {
        return;
    }
    let (scope_text, device) = (scope.to_string(), assets.device());
    let own = blocking(&assets.library, move |library| {
        let own: HashSet<String> = library.own_streams(&scope_text, &device)?.into_iter().collect();
        Ok(library.streams(&scope_text)?.into_iter().filter(|(stream, next)| *next > 0 && own.contains(stream)).collect::<Vec<_>>())
    })
    .await;
    let Ok(own) = own else { return };
    for (stream, next) in own {
        if !machine_orders(&stream) {
            continue;
        }
        let key = (hub.clone(), scope.to_string(), stream.clone());
        if !assets.direct.begun.lock().expect("begun").contains(&key) {
            let begin = Request::Begin { scope: scope.to_string(), stream: stream.clone() };
            if !matches!(ask(assets, link, begin).await, Ok(Reply::Epoch { .. })) {
                continue;
            }
            assets.direct.begun.lock().expect("begun").insert(key.clone());
        }
        if assets.direct.told.lock().expect("told").get(&key).is_some_and(|said| *said >= next) {
            continue;
        }
        let stirred = Request::Stirred { scope: scope.to_string(), stream, next };
        if matches!(ask(assets, link, stirred).await, Ok(Reply::Noted)) {
            assets.direct.told.lock().expect("told").insert(key, next);
        }
    }
}

/// Asks the machines that order what this one holds of `hub`'s
/// conversations where their streams stand: the scopes a stream of which
/// stands further there than here, to be gone through again. What catches a
/// word of moving lost on the way, a few asks however many scopes there
/// are.
pub(super) async fn look(assets: &Assets, hub: &HubId, link: &Arc<Link>) -> Result<BTreeSet<String>, String> {
    let mut behind = BTreeSet::new();
    if link.asset_version() < DIRECT {
        return Ok(behind);
    }
    let (theirs, device) = (hub.as_str().to_string(), assets.device());
    let paused = assets.copies.paused_now();
    let timeline = <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM;
    // Each conversation held whole here, the device that orders its
    // timeline, and where its streams stand here.
    let held = blocking(&assets.library, move |library| {
        let mut held = Vec::new();
        for (scope, _, _) in library.live_scopes_of(&theirs)? {
            if !wire::of_conversation(&scope) || paused.contains(&scope) || !library.is_pulled(&scope)? {
                continue;
            }
            let Some(orderer) = library.orderer(&scope, timeline)? else { continue };
            if orderer == device {
                continue;
            }
            let streams: HashMap<String, u64> = library.streams(&scope)?.into_iter().collect();
            held.push((scope, orderer, streams));
        }
        Ok(held)
    })
    .await?;
    let mut by_machine: HashMap<MachineName, Vec<String>> = HashMap::new();
    let mut here: HashMap<String, HashMap<String, u64>> = HashMap::new();
    {
        let view = link.view();
        for (scope, orderer, streams) in held {
            let Some(member) = view.by_key(&PublicKey::from_bytes(orderer)) else { continue };
            by_machine.entry(member.name.clone()).or_default().push(scope.clone());
            here.insert(scope, streams);
        }
    }
    for (machine, scopes) in by_machine {
        // A machine that does not answer holds up no other.
        let Ok(heads) = heads(assets, hub, link, &machine, scopes).await else { continue };
        for head in heads {
            let stands = here.get(&head.scope).and_then(|streams| streams.get(&head.stream)).copied().unwrap_or(0);
            if head.next > stands {
                behind.insert(head.scope);
            }
        }
    }
    Ok(behind)
}
