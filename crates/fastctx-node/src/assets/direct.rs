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
use super::session::{REVOKED, Trusted, ask, blocking};
use fastctx_content::v2::{EpochFact, Signed};
use fastctx_net::data::Stream;
use fastctx_net::link::{Feed, Link};
use fastctx_net::sign::PublicKey;
use fastctx_types::access::{HubId, Purpose};
use fastctx_types::asset_lines::{FRAME_BYTES, FRAMES, RUN_BYTES};
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
/// How long an exchange on a line may bring nothing, the line's opening
/// included, before it is given up.
const QUIET_FOR: Duration = Duration::from_secs(120);
/// How long asking where streams stand may take: little is carried.
const HEADS_WITHIN: Duration = Duration::from_secs(30);
/// The slowest a line is taken to carry, in bytes a second: a run of bytes
/// that comes whole or not at all may take as long as it would at this pace.
const SLOWEST: u64 = 32 * 1024;
/// How often an exchange under way is looked at for whether it still moves.
const LOOKED_AT_EVERY: Duration = Duration::from_secs(5);
/// How soon a line kept from before fails for having ended at its other
/// end: one that fails so is asked on once more, anew.
const ENDED_WITHIN: Duration = Duration::from_secs(3);
/// How long a machine that gave nothing when asked is left alone.
const LEFT_ALONE: Duration = Duration::from_secs(60);
/// How long a stream whose frames took it nowhere is not asked for again
/// from the same place, the first time; twice as long each time after.
const STALLED_FOR: Duration = Duration::from_secs(60);
/// How long what was heard of where a stream stands on its machine keeps
/// the stream's scope gone through again, the stream not there yet.
const OWED_FOR: Duration = Duration::from_secs(15 * 60);
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

/// A stream whose frames took it nowhere from a place.
struct Stall {
    from: u64,
    since: Instant,
    /// How many times running they did so, after the first.
    times: u32,
}

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
    /// Where each stream kept here stood as the hub was last told.
    told: Mutex<HashMap<(HubId, String, String), u64>>,
    /// Where streams held here were said to stand on the machines that keep
    /// them, each with when it was last said to stand further: what a scope
    /// is gone through again for until its streams are there.
    heard: Mutex<HashMap<(String, String), (u64, Instant)>>,
    /// The streams whose frames took them nowhere.
    stalled: Mutex<HashMap<(String, String), Stall>>,
    /// When the machines of each hub were last asked where their streams
    /// stand.
    looked: Mutex<HashMap<HubId, Instant>>,
    /// The objects wanted now that a machine keeps: the hub's not having
    /// one says nothing of whether its machine gives it the next time.
    kept_elsewhere: Mutex<HashSet<(HubId, [u8; 32])>>,
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
            told: Mutex::default(),
            heard: Mutex::default(),
            stalled: Mutex::default(),
            looked: Mutex::default(),
            kept_elsewhere: Mutex::default(),
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

    /// The hub was put back to an earlier time: what it was told of the
    /// streams kept here is told again, and begun again where it lost that.
    pub(super) fn put_back(&self, hub: &HubId) {
        self.told.lock().expect("told").retain(|(of, _, _), _| of != hub);
    }

    fn is_left_alone(&self, hub: &HubId, machine: &MachineName) -> bool {
        let mut left = self.left_alone.lock().expect("left alone");
        left.retain(|_, since| since.elapsed() < LEFT_ALONE);
        left.contains_key(&(hub.clone(), machine.clone()))
    }

    fn leave_alone(&self, hub: &HubId, machine: &MachineName) {
        self.left_alone.lock().expect("left alone").insert((hub.clone(), machine.clone()), Instant::now());
    }

    /// Whether what `stream` of `scope` took from `at` took it nowhere
    /// lately: it is not asked for again from there until a while passed,
    /// longer each time it went so.
    pub(super) fn stalled(&self, scope: &str, stream: &str, at: u64) -> bool {
        self.stalled
            .lock()
            .expect("stalled")
            .get(&(scope.to_string(), stream.to_string()))
            .is_some_and(|stall| stall.from == at && stall.since.elapsed() < STALLED_FOR * 2u32.pow(stall.times.min(6)))
    }

    /// Whether the frames of `stream` of `scope` from `at` took it nowhere
    /// before: asked for again, one frame comes, not a line's worth.
    pub(super) fn stalled_before(&self, scope: &str, stream: &str, at: u64) -> bool {
        self.stalled.lock().expect("stalled").get(&(scope.to_string(), stream.to_string())).is_some_and(|stall| stall.from == at)
    }

    /// `stream` of `scope` took nothing from `at`.
    pub(super) fn stall(&self, scope: &str, stream: &str, at: u64) {
        let mut stalled = self.stalled.lock().expect("stalled");
        let key = (scope.to_string(), stream.to_string());
        let times = stalled.get(&key).filter(|stall| stall.from == at).map_or(0, |stall| stall.times + 1);
        stalled.insert(key, Stall { from: at, since: Instant::now(), times });
    }

    /// `stream` of `scope` went on.
    pub(super) fn went_on(&self, scope: &str, stream: &str) {
        self.stalled.lock().expect("stalled").remove(&(scope.to_string(), stream.to_string()));
    }

    /// `stream` of `scope` was said to stand at `next` on the machine that
    /// keeps it, by that machine or by the hub it told.
    pub(super) fn heard(&self, scope: &str, stream: &str, next: u64) {
        let mut heard = self.heard.lock().expect("heard");
        let key = (scope.to_string(), stream.to_string());
        if heard.get(&key).is_none_or(|(stands, _)| *stands < next) {
            heard.insert(key, (next, Instant::now()));
        }
    }

    /// Whether a stream of `scope` was lately said to stand further on the
    /// machine that keeps it than it stands `here`. What is there by now,
    /// and what was said too long ago to go on asking for, is forgotten:
    /// the machines are asked again where their streams stand in time
    /// (`look`).
    pub(super) fn owed(&self, scope: &str, here: &HashMap<String, u64>) -> bool {
        let mut heard = self.heard.lock().expect("heard");
        heard.retain(|(of, stream), (next, at)| {
            of != scope || (here.get(stream).copied().unwrap_or(0) < *next && at.elapsed() < OWED_FOR)
        });
        heard.keys().any(|(of, _)| of == scope)
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

    /// The objects wanted of `hub` now that a machine keeps are `kept`.
    pub(super) fn keeps_elsewhere(&self, hub: &HubId, kept: impl IntoIterator<Item = [u8; 32]>) {
        let mut elsewhere = self.kept_elsewhere.lock().expect("kept elsewhere");
        elsewhere.retain(|(of, _)| of != hub);
        elsewhere.extend(kept.into_iter().map(|hash| (hub.clone(), hash)));
    }

    /// Whether the object `hash`, wanted of `hub`, is one a machine keeps.
    pub(super) fn is_kept_elsewhere(&self, hub: &HubId, hash: &[u8; 32]) -> bool {
        self.kept_elsewhere.lock().expect("kept elsewhere").contains(&(hub.clone(), *hash))
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

/// The machine that keeps `stream` of `scope`: the one that orders it,
/// where the stream stays there.
async fn keeper(assets: &Assets, link: &Link, scope: &str, stream: &str) -> Option<MachineName> {
    if !stays(link, scope, stream) {
        return None;
    }
    orderer(assets, link, scope, stream).await.ok().flatten()
}

/// The machine `stream` of `scope` is taken from: the one that keeps it,
/// where that machine is not left alone now.
pub(super) async fn source(assets: &Assets, hub: &HubId, link: &Link, scope: &str, stream: &str) -> Option<MachineName> {
    let machine = keeper(assets, link, scope, stream).await?;
    (!assets.direct.is_left_alone(hub, &machine)).then_some(machine)
}

/// Where `stream` of `scope` is to be taken from now, as a pull judges
/// whether it took all there is (`Source`).
pub(super) enum Source {
    /// No machine keeps it — the hub does, or the machine that ordered it
    /// is no longer of the network: what the hub holds is all there is.
    Hub,
    From(MachineName),
    /// The machine that keeps it is left alone now, or which one does could
    /// not be told: it may stand further than the hub.
    Away,
}

pub(super) async fn source_now(assets: &Assets, hub: &HubId, link: &Link, scope: &str, stream: &str) -> Source {
    if !stays(link, scope, stream) {
        return Source::Hub;
    }
    match orderer(assets, link, scope, stream).await {
        Ok(None) => Source::Hub,
        Ok(Some(machine)) if assets.direct.is_left_alone(hub, &machine) => Source::Away,
        Ok(Some(machine)) => Source::From(machine),
        Err(_) => Source::Away,
    }
}

/// An account logged in here that may ask `to` of the conversations it
/// reads there.
fn speaker(link: &Link, to: &MachineName) -> Option<AccountName> {
    let view = link.view();
    let me = view.me.clone();
    let logins: Vec<AccountName> = view.me().map(|member| member.logins.iter().cloned().collect()).unwrap_or_default();
    logins.into_iter().find(|account| view.admits(&me, account, to, Purpose::Agents).is_ok())
}

/// Asks the hub's word for `scopes`, as many as one word names at most. A
/// word for one scope is kept and used again; one for many goes with the
/// one ask it was made for, and no ask of one scope carries the others.
async fn permits(assets: &Assets, hub: &HubId, link: &Arc<Link>, scopes: Vec<String>) -> Result<Arc<Permit>, String> {
    let alone = scopes.len() == 1;
    let Reply::Permits { until, granted, word } = ask(assets, link, Request::Permits { scopes }).await? else {
        return Err("the hub did not say what this machine holds".into());
    };
    let permit = Arc::new(Permit { until, granted, word });
    if alone {
        let mut kept = assets.direct.permits.lock().expect("permits");
        let now = assets.direct.hub_now(hub);
        kept.retain(|_, permit| permit.until > now);
        for scope in &permit.granted {
            kept.insert((hub.clone(), scope.clone()), permit.clone());
        }
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

/// The machine asked did not take the hub's word for `scope` — the hub
/// changed the key its word is under, or that machine has no session with
/// it yet: another word is asked for the next time.
fn word_refused(assets: &Assets, hub: &HubId, scope: &str, why: &str) {
    if why.contains(REFUSED) {
        assets.direct.permits.lock().expect("permits").remove(&(hub.clone(), scope.to_string()));
    }
}

/// A line to a machine: the data connection of a call it answers
/// (`RemoteCall::Assets`), one exchange on it at a time.
struct Line {
    jobs: mpsc::Sender<Job>,
}

/// When an exchange under way last brought something.
type Moving = Arc<Mutex<Instant>>;

fn moves(moving: &Moving) {
    *moving.lock().expect("moving") = Instant::now();
}

/// One exchange asked on a line, and where its answer goes.
enum Job {
    Heads { ask: Ask, moving: Moving, answer: oneshot::Sender<Result<Vec<Head>, String>> },
    Frames { ask: Ask, moving: Moving, answer: oneshot::Sender<Result<Vec<wire::Frame>, String>> },
    Objects { ask: Ask, wanted: Vec<wire::ObjectRef>, moving: Moving, answer: oneshot::Sender<Result<Vec<Option<Vec<u8>>>, String>> },
}

/// What a machine said where an answer of another kind was due.
fn refused(told: Told) -> String {
    match told {
        Told::Failed { why } => format!("{REFUSED}{why}"),
        Told::Busy => BUSY.into(),
        _ => "it answered with something else".into(),
    }
}

async fn heads_on(stream: &mut Stream, ask: &Ask, moving: &Moving) -> Result<Vec<Head>, String> {
    stream.tell(ask).await?;
    moves(moving);
    match stream.told::<Told>("where a machine's streams stand").await? {
        Told::Heads { heads } => Ok(heads),
        other => Err(refused(other)),
    }
}

async fn frames_on(stream: &mut Stream, ask: &Ask, moving: &Moving) -> Result<Vec<wire::Frame>, String> {
    stream.tell(ask).await?;
    moves(moving);
    let mut frames = match stream.told::<Told>("a machine's frames").await? {
        Told::Frames { frames } => frames,
        other => return Err(refused(other)),
    };
    moves(moving);
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

/// Takes the objects `wanted` as they come, each whole into `given`: those
/// that came before the line failed are there all the same.
async fn objects_on(
    stream: &mut Stream,
    ask: &Ask,
    wanted: &[wire::ObjectRef],
    moving: &Moving,
    given: &mut Vec<Option<Vec<u8>>>,
) -> Result<(), String> {
    stream.tell(ask).await?;
    moves(moving);
    for (index, object) in wanted.iter().enumerate() {
        match stream.told::<Told>("a machine's word on an object").await? {
            Told::Object { index: said, given: true, .. } if said as usize == index => {
                let mut bytes = Vec::with_capacity(object.size as usize);
                while (bytes.len() as u64) < object.size {
                    moves(moving);
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
        moves(moving);
    }
    Ok(())
}

/// Carries a line: each exchange asked on it in turn, until one fails or
/// nothing is asked for a while. An exchange the one that asked waits for
/// no more is let go where it is. Dropping the connection ends the call.
async fn carry(mut stream: Stream, mut jobs: mpsc::Receiver<Job>) -> Result<(), String> {
    while let Ok(Some(job)) = tokio::time::timeout(KEPT_FOR, jobs.recv()).await {
        let went = match job {
            Job::Heads { ask, moving, mut answer } => {
                let done = tokio::select! {
                    done = heads_on(&mut stream, &ask, &moving) => done,
                    () = answer.closed() => break,
                };
                let went = done.is_ok();
                let _ = answer.send(done);
                went
            }
            Job::Frames { ask, moving, mut answer } => {
                let done = tokio::select! {
                    done = frames_on(&mut stream, &ask, &moving) => done,
                    () = answer.closed() => break,
                };
                let went = done.is_ok();
                let _ = answer.send(done);
                went
            }
            Job::Objects { ask, wanted, moving, mut answer } => {
                let mut given = Vec::with_capacity(wanted.len());
                let done = tokio::select! {
                    done = objects_on(&mut stream, &ask, &wanted, &moving, &mut given) => done,
                    () = answer.closed() => break,
                };
                let went = done.is_ok();
                let _ = answer.send(match done {
                    Err(why) if given.is_empty() => Err(why),
                    // Those that came whole are kept; the rest are asked
                    // for again.
                    _ => {
                        given.resize_with(wanted.len(), || None);
                        Ok(given)
                    }
                });
                went
            }
        };
        // What was said and sent part way cannot be told from the next
        // exchange: the line ends.
        if !went {
            break;
        }
    }
    Ok(())
}

/// A line to `to`: one open that nothing is asked on, or a new one; whether
/// it was kept from before.
fn line(assets: &Assets, hub: &HubId, link: &Arc<Link>, to: &MachineName) -> Result<(Line, bool), String> {
    let kept = assets.direct.lines.lock().expect("lines").get_mut(&(hub.clone(), to.clone())).and_then(|lines| {
        lines.retain(|line| !line.jobs.is_closed());
        lines.pop()
    });
    if let Some(kept) = kept {
        return Ok((kept, true));
    }
    let account = speaker(link, to).ok_or_else(|| format!("no account logged in here may ask {to} of its conversations"))?;
    let (jobs, taken) = mpsc::channel::<Job>(1);
    let feed: Feed = Box::new(move |stream| Box::pin(carry(stream, taken)));
    let (link, to) = (link.clone(), to.clone());
    fastctx_platform::task::spawn(async move {
        let _ = link.call_with_stream(&to, &account, Purpose::Agents, RemoteCall::Assets, feed).await;
    });
    Ok((Line { jobs }, false))
}

/// Asks one exchange on `line` and waits for its answer for as long as the
/// exchange brings something every `quiet` at least.
async fn asked_on<T>(
    line: &Line,
    to: &MachineName,
    quiet: Duration,
    job: &impl Fn(Moving, oneshot::Sender<Result<T, String>>) -> Job,
) -> Result<T, String> {
    let moving: Moving = Arc::new(Mutex::new(Instant::now()));
    let (answer, answered) = oneshot::channel();
    line.jobs.send(job(moving.clone(), answer)).await.map_err(|_| format!("the line to {to} ended"))?;
    tokio::pin!(answered);
    loop {
        tokio::select! {
            done = &mut answered => return done.unwrap_or_else(|_| Err(format!("{to} did not take the line"))),
            () = tokio::time::sleep(LOOKED_AT_EVERY) => {
                let still = moving.lock().expect("moving").elapsed();
                // Waited for no more, the exchange is let go on the line
                // (`carry`), and the line with it.
                if still > quiet {
                    return Err(format!("{to} brought nothing for {} s", still.as_secs()));
                }
            }
        }
    }
}

/// Asks one exchange of `to` on a line and waits for its answer; the line
/// is kept for the next where it went well, and the machine left alone a
/// while where it did not.
async fn exchange<T>(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    to: &MachineName,
    quiet: Duration,
    job: impl Fn(Moving, oneshot::Sender<Result<T, String>>) -> Job,
) -> Result<T, String> {
    if assets.direct.is_left_alone(hub, to) {
        return Err(format!("{UNREACHED}{to} gave nothing lately and is left alone for now"));
    }
    let mut once_more = true;
    loop {
        let (line, kept) = line(assets, hub, link, to).map_err(|why| format!("{UNREACHED}{why}"))?;
        let began = Instant::now();
        let done = asked_on(&line, to, quiet, &job).await;
        match &done {
            Ok(_) => assets.direct.lines.lock().expect("lines").entry((hub.clone(), to.clone())).or_default().push(line),
            // One that refused this ask answers others: only one that did
            // not answer, or is busy, is left alone a while.
            Err(why) if why.starts_with(REFUSED) => {}
            // A line kept from before may have ended at its other end since.
            Err(_) if kept && once_more && began.elapsed() < ENDED_WITHIN => {
                once_more = false;
                // The others kept from before ended with it, as likely as
                // not: it is asked on a line made now.
                assets.direct.lines.lock().expect("lines").remove(&(hub.clone(), to.clone()));
                continue;
            }
            Err(_) => assets.direct.leave_alone(hub, to),
        }
        return done.map_err(|why| format!("{UNREACHED}{to}: {why}"));
    }
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
        // Of the scopes meant now alone, of those the hub's word names.
        let asked: Vec<String> = some.iter().filter(|scope| permit.granted.contains(scope)).cloned().collect();
        if asked.is_empty() {
            continue;
        }
        let ask = Ask::Heads { permit: (*permit).clone(), scopes: asked.clone() };
        let told = exchange(assets, hub, link, machine, HEADS_WITHIN, |moving, answer| Job::Heads { ask: ask.clone(), moving, answer }).await;
        let told = match told {
            Ok(told) => told,
            Err(why) => {
                for scope in &asked {
                    word_refused(assets, hub, scope, &why);
                }
                return Err(why);
            }
        };
        // A machine says here where the scopes asked of stand, and of no
        // other: what it said of another is not heard.
        heads.extend(told.into_iter().filter(|head| asked.contains(&head.scope)));
    }
    Ok(heads)
}

/// Up to `most` frames of `stream` of `scope` from the one holding `from`,
/// each whole, taken from `machine`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn frames(
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    machine: &MachineName,
    scope: &str,
    stream: &str,
    from: u64,
    most: usize,
) -> Result<Vec<wire::Frame>, String> {
    let permit = permit(assets, hub, link, scope).await.map_err(|why| format!("{UNREACHED}{why}"))?;
    let ask = Ask::Frames {
        permit: (*permit).clone(),
        scope: scope.to_string(),
        stream: stream.to_string(),
        from,
        frames: most.clamp(1, FRAMES) as u32,
    };
    // The frames' bodies come as one run, whole or not at all.
    let quiet = QUIET_FOR + Duration::from_secs((FRAME_BYTES as u64 + APART_FRAME_BYTES) / SLOWEST);
    let given = exchange(assets, hub, link, machine, quiet, |moving, answer| Job::Frames { ask: ask.clone(), moving, answer }).await;
    if let Err(why) = &given {
        word_refused(assets, hub, scope, why);
    }
    given
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
    // An object's bytes come a run at a time.
    let quiet = QUIET_FOR + Duration::from_secs(RUN_BYTES / SLOWEST);
    let given = exchange(assets, hub, link, machine, quiet, |moving, answer| Job::Objects {
        ask: ask.clone(),
        wanted: objects.clone(),
        moving,
        answer,
    })
    .await;
    if let Err(why) = &given {
        word_refused(assets, hub, scope, why);
    }
    given
}

/// The machine that keeps an object `scope`'s frames refer to — the one
/// that orders a stream it is read through here, where that stream stays
/// on its machine — and whether it is asked now: not while it is left
/// alone.
pub(super) async fn object_source(assets: &Assets, hub: &HubId, link: &Link, scope: &str, hash: [u8; 32]) -> Option<(MachineName, bool)> {
    if !kept_by_machines(link, scope) {
        return None;
    }
    let scope_text = scope.to_string();
    let read = blocking(&assets.library, move |library| library.streams_reading(&scope_text, &hash)).await.ok()?;
    // One that is asked now before one left alone: an object two machines
    // keep comes from whichever gives it.
    let mut left_alone = None;
    for stream in read {
        if let Some(machine) = keeper(assets, link, scope, &stream).await {
            if !assets.direct.is_left_alone(hub, &machine) {
                return Some((machine, true));
            }
            left_alone.get_or_insert(machine);
        }
    }
    left_alone.map(|machine| (machine, false))
}

/// Has the hub sign that this machine orders `stream` of `scope` — its
/// first epoch, or the one it orders in already — and keeps the hub's word
/// here: what a machine keeps of a stream known here for its own by the
/// hub's word is never taken for written in vain (`Library::unaccepted`).
async fn begin(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str) -> Result<(), String> {
    let request = Request::Begin { scope: scope.to_string(), stream: stream.to_string() };
    let Reply::Epoch { descriptor } = ask(assets, link, request).await? else {
        return Err("the hub did not begin the stream".into());
    };
    if !assets.heard(hub, key, &descriptor.fact.0, &descriptor.word.0) {
        return Err("the epoch's descriptor is not the hub's".into());
    }
    let fact = EpochFact::decode(&descriptor.fact.0)?;
    if fact.scope != scope || fact.stream != stream || fact.device != assets.device() {
        return Err("the hub began another stream than the one asked for".into());
    }
    let signed = Signed { fact: descriptor.fact.0, signature: descriptor.word.0 };
    let scope_text = scope.to_string();
    let (epoch, start) = (fact.epoch, fact.start);
    // The epoch this machine writes the stream in is the one the hub has it
    // ordering in, under the very descriptor the hub signed; a later epoch
    // of this machine's — handed to it, or what it kept set back to before
    // it — is opened here only where the stream stands exactly where that
    // epoch begins, chained to what it follows: what stands short of it,
    // goes past it, or came to it by another chain is not this machine's to
    // go on from, and the hub is told nothing of the stream until that is
    // put right. Looked at and opened in one transaction (`begin_epoch`).
    let begun = blocking(&assets.library, move |library| library.begin_epoch(&scope_text, signed, &Trusted)).await?;
    match begun {
        fastctx_content::v2::Begun::InForce => Ok(()),
        fastctx_content::v2::Begun::Opened => {
            assets.taken_over.lock().expect("taken over").push(scope.to_string());
            Ok(())
        }
        fastctx_content::v2::Begun::Refused(why) => Err(format!(
            "the hub has this machine ordering it in epoch {epoch}, from {start}, but {why}: the hub's word on it is not taken here"
        )),
    }
}

/// Tells the hub where the streams of `scope` this machine orders and keeps
/// stand, each that moved since the hub was last told; one whose epoch is
/// not known here by the hub's word, or that the hub knows no epoch of — it
/// was put back to before it — is begun there first. What could not be told
/// is said, so the scope goes up again the next pass: the machines that
/// hold copies hear of a stream through the hub alone.
pub(super) async fn announce(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str) -> Result<(), String> {
    if !kept_by_machines(link, scope) {
        return Ok(());
    }
    let (scope_text, device) = (scope.to_string(), assets.device());
    let own = blocking(&assets.library, move |library| {
        let own: HashSet<String> = library.own_streams(&scope_text, &device)?.into_iter().collect();
        Ok(library.streams(&scope_text)?.into_iter().filter(|(stream, next)| *next > 0 && own.contains(stream)).collect::<Vec<_>>())
    })
    .await?;
    let mut failed: Option<String> = None;
    for (stream, next) in own {
        if !machine_orders(&stream) {
            continue;
        }
        let place = (hub.clone(), scope.to_string(), stream.clone());
        if assets.direct.told.lock().expect("told").get(&place).is_some_and(|said| *said >= next) {
            continue;
        }
        // Said with the epoch it is written in here, not merely some epoch of
        // it, as the hub's word on it kept here says it: the hub takes it
        // only where that is its own last. Where it is not known here, or the
        // hub takes it not, it is begun there first.
        let in_force = || {
            let (scope_text, stream_text) = (scope.to_string(), stream.clone());
            blocking(&assets.library, move |library| library.epoch_in_force(&scope_text, &stream_text))
        };
        let stirred = |fact: EpochFact| Request::Stirred {
            scope: scope.to_string(),
            stream: stream.clone(),
            next,
            epoch: Some(wire::StirredIn { epoch: fact.epoch, start: fact.start, follows: wire::Bytes::from(fact.follows) }),
        };
        let mut said = match in_force().await? {
            Some(fact) => ask(assets, link, stirred(fact)).await,
            None => Err("its epoch is not known here by the hub's word yet".to_string()),
        };
        if said.as_ref().is_err_and(|why| !why.starts_with(REVOKED)) {
            said = match begin(assets, hub, link, key, scope, &stream).await {
                Ok(()) => match in_force().await? {
                    Some(fact) => ask(assets, link, stirred(fact)).await,
                    None => Err("its epoch is not known here by the hub's word after it was begun".to_string()),
                },
                Err(why) => Err(why),
            };
        }
        match said {
            Ok(Reply::Noted) => {
                assets.direct.told.lock().expect("told").insert(place, next);
            }
            Ok(_) => {
                failed.get_or_insert(format!("{stream}: the hub did not note where it stands"));
            }
            Err(why) if why.starts_with(REVOKED) => return Err(why),
            Err(why) => {
                failed.get_or_insert(format!("telling the hub where {stream} stands: {why}"));
            }
        }
    }
    failed.map_or(Ok(()), Err)
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
    // Each conversation held whole here, the devices that order streams of
    // it — its timeline's, and those of streams begun on other machines —
    // and where its streams stand here.
    let held = blocking(&assets.library, move |library| {
        let mut held = Vec::new();
        for (scope, _, _) in library.live_scopes_of(&theirs)? {
            if !wire::of_conversation(&scope) || paused.contains(&scope) || !library.is_pulled(&scope)? {
                continue;
            }
            let streams: HashMap<String, u64> = library.streams(&scope)?.into_iter().collect();
            let mut orderers = BTreeSet::new();
            for stream in streams.keys().map(String::as_str).chain([timeline]) {
                if machine_orders(stream)
                    && let Some(orderer) = library.orderer(&scope, stream)?
                    && orderer != device
                {
                    orderers.insert(orderer);
                }
            }
            if !orderers.is_empty() {
                held.push((scope, orderers, streams));
            }
        }
        Ok(held)
    })
    .await?;
    let mut by_machine: HashMap<MachineName, Vec<String>> = HashMap::new();
    let mut here: HashMap<String, HashMap<String, u64>> = HashMap::new();
    {
        let view = link.view();
        for (scope, orderers, streams) in held {
            for orderer in orderers {
                let Some(member) = view.by_key(&PublicKey::from_bytes(orderer)) else { continue };
                by_machine.entry(member.name.clone()).or_default().push(scope.clone());
            }
            here.insert(scope, streams);
        }
    }
    for (machine, scopes) in by_machine {
        // A machine that does not answer holds up no other.
        let Ok(heads) = heads(assets, hub, link, &machine, scopes).await else { continue };
        for head in heads {
            let stands = here.get(&head.scope).and_then(|streams| streams.get(&head.stream)).copied().unwrap_or(0);
            if head.next > stands {
                assets.direct.heard(&head.scope, &head.stream, head.next);
                behind.insert(head.scope);
            }
        }
    }
    Ok(behind)
}
