//! A device's copy of what it may hold of each hub's assets (design §29
//! 3.2): downloaded only, never ordered here. A light device (a phone, by
//! default) holds each stream as the registry's `mobile` column has its kind
//! (`tiers`); a whole one takes everything by its frames from the first. Its
//! library is a node's (`fastctx_content::v2::Library`), in `replica/` of
//! the device's directory, with what it keeps of each hub beside it
//! (`pins`). It follows each link the device has while it is up: a whole
//! look now and then, otherwise the streams the hub says moved and what
//! changed in each owner library since it stood there, telling the hub
//! where it stands (`standing`).

mod objects;
mod pass;
mod pins;
mod standing;
mod tiers;

pub use fastctx_content::v2::{Library, OlderTaken};
pub use tiers::Tier;

use fastctx_net::ids::HubId;
use fastctx_net::link::{Link, Status};
use fastctx_types::assets::Push;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// What a scope's pass ends with when the hub says the device reads it no
/// more: not a failure.
pub(super) const REVOKED: &str = "no longer read here: ";

/// What a request ends with when the hub keeps every asset sealed.
pub const SEALED: &str = "the hub keeps every asset sealed: ";

/// How often every scope is looked at whole, and how long a follower waits
/// when nothing calls for it.
const LOOK_WHOLE: Duration = Duration::from_secs(300);
const AGAIN: Duration = Duration::from_secs(60);

/// Runs `work` on the library off the async threads.
pub(super) async fn blocking<T: Send + 'static>(
    library: &Library,
    work: impl FnOnce(&Library) -> fastctx_content::Result<T> + Send + 'static,
) -> Result<T, String> {
    let library = library.clone();
    fastctx_platform::task::spawn_blocking(move || work(&library))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// Each object a hub did not give, by hub and hash: when it last did not,
/// and how many times.
type Unreached = HashMap<(HubId, [u8; 32]), (Instant, u32)>;

/// What a follower of one hub was told to look at.
#[derive(Default)]
struct Called {
    /// Streams the hub said moved: scope, stream and where it stands.
    moved: BTreeMap<(String, String), u64>,
    /// Something calls for a whole look: a fact, a scope let go.
    whole: bool,
}

pub struct Replica {
    library: Library,
    device: [u8; 32],
    /// The tier it holds as now: switched while it runs (`set_tier`).
    tier: std::sync::atomic::AtomicU8,
    pins: pins::Pins,
    /// The scopes a hub said the device reads no more since it started:
    /// a reader is not sent to ask their machines instead (§28.39).
    revoked: Mutex<BTreeSet<String>>,
    /// Each object a hub did not give, when it last did not and how often.
    unreached: Mutex<Unreached>,
    /// The link followed for each hub: a follower of any other goes.
    links: Mutex<BTreeMap<HubId, Weak<Link>>>,
    called: Mutex<BTreeMap<HubId, Called>>,
    /// What wakes each hub's follower; a wake while it works is kept.
    wakes: Mutex<BTreeMap<HubId, Arc<tokio::sync::Notify>>>,
    /// How each hub's last pass ended, and how many passes ended in all.
    ended: Mutex<BTreeMap<HubId, Result<(), String>>>,
    passes: tokio::sync::watch::Sender<u64>,
    /// What the device last told each hub of where it stands, and when.
    reported: Mutex<HashMap<HubId, (fastctx_types::sync_status::SyncReport, Instant)>>,
}

impl Replica {
    /// Opens the copy in `dir` for the device whose public key is `device`,
    /// holding as `tier` holds. Call on a thread that may block.
    pub fn open(dir: &Path, device: [u8; 32], tier: Tier) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(dir)?;
        let (library, _) = Library::open(dir).map_err(|error| io::Error::other(error.to_string()))?;
        // The kinds a node knows: records of others wait in their frames,
        // out of the index, until a release that knows them (§2.3).
        let kinds: Arc<dyn fastctx_content::v2::Authorizer> = Arc::new(
            fastctx_content::v2::Kinds::new()
                .register::<fastctx_content::v2::Timeline>()
                .register::<fastctx_content::v2::ConversationMeta>()
                .register::<fastctx_content::v2::ConversationWords>()
                .register::<fastctx_content::v2::ConversationAttachment>()
                .register::<fastctx_content::v2::JobFacts>()
                .register::<fastctx_content::v2::JobOutput>()
                .register::<fastctx_content::v2::ConversationPlan>()
                .register::<fastctx_content::v2::ConversationHistory>()
                .register::<fastctx_content::v2::ConversationGoal>()
                .register::<fastctx_content::v2::ConversationSentinel>()
                .register::<fastctx_content::v2::AccountSettings>()
                .register::<fastctx_content::v2::AccountSkill>()
                .register::<fastctx_content::v2::ProjectLedger>()
                .register::<fastctx_content::v2::MachineAudit>()
                .register::<fastctx_content::v2::AccountNotice>()
                .register::<fastctx_content::v2::MachineLog>()
                .register::<fastctx_content::v2::McpLog>()
                .register::<fastctx_content::v2::HubLog>()
                .register::<fastctx_content::v2::ConversationTimer>()
                .register::<fastctx_content::v2::ConversationSentinelRun>()
                .register::<fastctx_content::v2::ConversationArtifact>()
                .register::<fastctx_content::v2::AccountProfile>()
                .register::<fastctx_content::v2::AccountCollaboration>()
                .register::<fastctx_content::v2::AccountActivity>()
                .register::<fastctx_content::v2::AccountUsage>()
                .register::<fastctx_content::v2::ConversationDraft>()
                .register::<fastctx_content::v2::ConversationFileChange>(),
        );
        library.know(kinds.clone());
        library.reconsider(kinds, true).map_err(|error| io::Error::other(error.to_string()))?;
        // A window being installed as the device stopped is finished first.
        library.resume_installs().map_err(|error| io::Error::other(error.to_string()))?;
        let pins = pins::Pins::open(dir.join("hubs.json"))?;
        Ok(Self {
            library,
            device,
            tier: std::sync::atomic::AtomicU8::new(tier.code()),
            pins,
            revoked: Mutex::default(),
            unreached: Mutex::default(),
            links: Mutex::default(),
            called: Mutex::default(),
            wakes: Mutex::default(),
            ended: Mutex::default(),
            passes: tokio::sync::watch::channel(0).0,
            reported: Mutex::default(),
        })
    }

    fn device(&self) -> [u8; 32] {
        self.device
    }

    pub fn tier(&self) -> Tier {
        Tier::of_code(self.tier.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Holds as `tier` holds from now on: each hub is looked at whole at
    /// once. What a lighter tier no longer holds stays until it is let go
    /// with its holding period.
    pub fn set_tier(&self, tier: Tier) {
        if self.tier.swap(tier.code(), std::sync::atomic::Ordering::SeqCst) == tier.code() {
            return;
        }
        // Where it stood, and what it pulled whole, was as the other tier
        // holds: a heavier one holds what that one never took, so the device
        // stands nowhere until its whole looks of each library end anew
        // (design §28.42). Not done, a look stands it again all the same.
        for hub in self.library.joined().unwrap_or_default() {
            let _ = self.library.forget_feeds(&hub);
            let _ = self.library.forget_pulled(&hub);
        }
        self.reported.lock().expect("reported").clear();
        let hubs: Vec<HubId> = self.links.lock().expect("links").keys().cloned().collect();
        for hub in hubs {
            self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
            self.wake(&hub).notify_one();
        }
    }

    /// Whether a hub said the device reads `scope` no more since it started.
    pub fn revoked(&self, scope: &str) -> bool {
        self.revoked.lock().expect("revoked").contains(scope)
    }

    /// Keeps that a hub said the device reads `scope` no more.
    pub(super) fn note_revoked(&self, scope: &str) {
        self.revoked.lock().expect("revoked").insert(scope.to_string());
    }

    /// The library the copy is in, for reading.
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Moves each time a pass with a hub ends.
    pub fn passes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.passes.subscribe()
    }

    /// How the last pass with `hub` ended.
    pub fn ended(&self, hub: &HubId) -> Option<Result<(), String>> {
        self.ended.lock().expect("ended").get(hub).cloned()
    }

    /// Follows `link` while it is up and the device on its network: on a
    /// task of its own, one for each link however often it is asked.
    pub(crate) fn follow(self: &Arc<Self>, link: &Arc<Link>) {
        let hub = link.membership().id.clone();
        {
            let mut links = self.links.lock().expect("links");
            if links.get(&hub).and_then(Weak::upgrade).is_some_and(|followed| Arc::ptr_eq(&followed, link)) {
                return;
            }
            links.insert(hub.clone(), Arc::downgrade(link));
        }
        self.wakes.lock().expect("wakes").entry(hub.clone()).or_default();
        self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
        let (replica, link) = (self.clone(), Arc::downgrade(link));
        tokio::spawn(async move { replica.following(hub, link).await });
    }

    /// Whether `link` is the one followed for `hub`: one replaced, or left,
    /// is not, and its follower writes nothing more.
    fn current(&self, hub: &HubId, link: &Arc<Link>) -> bool {
        self.links
            .lock()
            .expect("links")
            .get(hub)
            .and_then(Weak::upgrade)
            .is_some_and(|followed| Arc::ptr_eq(&followed, link))
    }

    fn wake(&self, hub: &HubId) -> Arc<tokio::sync::Notify> {
        self.wakes.lock().expect("wakes").entry(hub.clone()).or_default().clone()
    }

    async fn following(self: Arc<Self>, hub: HubId, link: Weak<Link>) {
        // What a purge left before the device stopped goes on, online or not.
        let _ = self.upkeep().await;
        let mut looked: Option<Instant> = None;
        loop {
            let Some(up) = link.upgrade() else { return };
            if !self.current(&hub, &up) {
                return;
            }
            // The hub removed the device: what it held of the network goes.
            if !up.membership_active() {
                let _ = self.left(&hub).await;
                return;
            }
            // Stopped here: nothing to follow until a link is started again.
            if matches!(up.status(), Status::Stopped { .. }) {
                return;
            }
            if !up.online_within(Duration::from_secs(30)).await {
                continue;
            }
            let called = std::mem::take(self.called.lock().expect("called").entry(hub.clone()).or_default());
            let whole = called.whole || looked.is_none_or(|at| at.elapsed() >= LOOK_WHOLE);
            let ended =
                if whole { self.pass_whole(&hub, &up).await } else { self.pass_moved(&hub, &up, called.moved).await };
            let more = match &ended {
                Ok(more) => *more,
                Err(_) => false,
            };
            if whole && ended.is_ok() {
                looked = Some(Instant::now());
            }
            // What a turn left goes on at once, as a whole look: streams
            // past their turn, a backfill, objects waiting.
            if more {
                self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
            }
            self.ended.lock().expect("ended").insert(hub.clone(), ended.map(|_| ()));
            self.passes.send_modify(|passes| *passes += 1);
            drop(up);
            if !more {
                let _ = tokio::time::timeout(AGAIN, self.wake(&hub).notified()).await;
            }
        }
    }

    /// Looks at every scope held of `hub`: whether more is left to take.
    async fn pass_whole(&self, hub: &HubId, link: &Arc<Link>) -> Result<bool, String> {
        let library = &self.library;
        let key = pass::open(self, hub, link).await?;
        if !self.current(hub, link) {
            return Err("the link is no longer the one followed".into());
        }
        let (theirs, device) = (hub.as_str().to_string(), self.device());
        let network = blocking(library, move |library| library.join(&theirs, &device, false)).await?;
        // Where each owner library held here stands, asked before what the
        // device may hold is listed: a scope begun after that is in the
        // library's changes past the mark, and one begun before is listed,
        // so none falls between (codex-c8f08e64 #2).
        let theirs = hub.as_str().to_string();
        let owners: BTreeSet<String> = blocking(library, move |library| library.live_scopes_of(&theirs))
            .await?
            .iter()
            .filter_map(|(_, descriptor, _)| standing::owner_of(descriptor))
            .collect();
        let mut failed = None;
        let marks = match standing::tops(self, link, &owners).await {
            Ok(marks) => Some(marks),
            Err(why) => {
                failed = Some(format!("where the owner libraries stand: {why}"));
                None
            }
        };
        // Stood in only on a listing whole and settled: a scope not listed is
        // one the device may lack.
        let listed_whole = match pass::discover(self, hub, link, &key, &network).await {
            Ok(listed) => listed.is_some(),
            Err(why) => {
                failed.get_or_insert(why);
                false
            }
        };
        let theirs = hub.as_str().to_string();
        let held = blocking(library, move |library| library.live_scopes_of(&theirs)).await?;
        let mut short = BTreeSet::new();
        let mut more = false;
        for (scope, descriptor, holding) in held {
            let owner = standing::owner_of(&descriptor);
            match pass::pull(self, hub, link, &key, &scope, holding).await {
                // Pulled whole in its holding period, as the tier holds it:
                // its changes carry it from now on.
                Ok(false) => {
                    let scope = scope.clone();
                    blocking(library, move |library| library.pulled(&scope, holding)).await?;
                }
                Ok(true) => {
                    more = true;
                    short.extend(owner);
                }
                Err(why) if why.starts_with(REVOKED) => {}
                Err(why) => {
                    short.extend(owner);
                    failed.get_or_insert(format!("{scope}: {why}"));
                }
            }
        }
        if let Some(marks) = marks.filter(|_| listed_whole) {
            standing::stand(self, hub, marks, &short).await?;
        }
        let (fetched, left) = objects::fetch(self, hub, link).await;
        if let Some(why) = fetched.into_iter().next() {
            failed.get_or_insert(why);
        }
        self.upkeep().await?;
        // Told as it stands now; a hub that does not keep it is told again.
        let _ = standing::report(self, hub, link).await;
        match failed {
            Some(why) => Err(why),
            None => Ok(more || left),
        }
    }

    /// Takes what the hub said moved of `hub`'s streams.
    async fn pass_moved(
        &self,
        hub: &HubId,
        link: &Arc<Link>,
        moved: BTreeMap<(String, String), u64>,
    ) -> Result<bool, String> {
        let library = &self.library;
        let theirs = hub.as_str().to_string();
        let stood = blocking(library, move |library| library.feed_owners(&theirs)).await?;
        if moved.is_empty() && stood.is_empty() {
            return Ok(false);
        }
        let key = pass::open(self, hub, link).await?;
        let mut more = false;
        let scopes: BTreeSet<String> = moved.keys().map(|(scope, _)| scope.clone()).collect();
        for scope in scopes {
            let key_text = scope.clone();
            let Some(holding) = blocking(library, move |library| library.holding(&key_text)).await? else {
                // Not held here yet: the hub may list it now.
                self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
                continue;
            };
            let key_text = scope.clone();
            let here: BTreeMap<String, u64> =
                blocking(library, move |library| library.streams(&key_text)).await?.into_iter().collect();
            for ((_, stream), next) in moved.iter().filter(|((of, _), _)| *of == scope) {
                // A stream first heard of may stand for another the hub
                // gives this device no more (§29, 2.4): a whole look lists
                // what it withholds.
                if !here.contains_key(stream) {
                    self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
                }
                let at = here.get(stream).copied().unwrap_or(0);
                match pass::hold(self, hub, link, &key, &scope, stream, holding, at, *next, false).await {
                    Ok(went_on) => more |= went_on,
                    // A new epoch, or a scope let go: a whole look sorts it.
                    Err(_) => self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true,
                }
            }
        }
        // What changed in each library the device stands in, by its change
        // numbers, a push lost on the way among it: where it stands moves
        // once all of it is taken (design §28.1).
        for owner in stood {
            match standing::feed(self, hub, link, &key, &owner).await {
                Ok(went_on) => more |= went_on,
                Err(_) => self.call_whole(hub),
            }
        }
        let (_, left) = objects::fetch(self, hub, link).await;
        let _ = standing::report(self, hub, link).await;
        Ok(more || left || self.called.lock().expect("called").get(hub).is_some_and(|called| called.whole))
    }

    /// Something calls for a whole look of `hub` at the next pass.
    fn call_whole(&self, hub: &HubId) {
        self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
    }

    /// Lets go of what scopes let go held, and of objects nothing holds.
    async fn upkeep(&self) -> Result<(), String> {
        let library = &self.library;
        while blocking(library, |library| library.purging()).await? {
            blocking(library, |library| library.purge()).await?;
        }
        while blocking(library, |library| library.collect()).await?.objects > 0 {}
        Ok(())
    }

    /// The hub `hub` told the device something unasked. A scope it reads no
    /// more ends its holding at once, as a node's does (§6.3).
    pub(crate) fn pushed(&self, hub: &HubId, push: Push) {
        {
            let mut called = self.called.lock().expect("called");
            let called = called.entry(hub.clone()).or_default();
            match push {
                Push::Moved { scope, stream, next } => {
                    let at = called.moved.entry((scope, stream)).or_insert(next);
                    *at = (*at).max(next);
                }
                Push::Revoked { scope } => {
                    called.whole = true;
                    self.note_revoked(&scope);
                    let (library, device) = (self.library.clone(), self.device());
                    tokio::task::spawn_blocking(move || library.revoke_keeping(&scope, &device));
                }
                Push::Fact { .. } => called.whole = true,
                Push::Checkpointed { .. } => return,
            }
        }
        self.wake(hub).notify_one();
    }

    /// `hub`'s directory changed: what the device may hold of a scope it
    /// holds may have changed with it — a project's ledger under the
    /// machines its account may use now (§29, 2.4) — so the next pass looks
    /// whole, with what the hub withholds now (codex-5c30d5e9 R03).
    pub(crate) fn directory_changed(&self, hub: &HubId) {
        self.called.lock().expect("called").entry(hub.clone()).or_default().whole = true;
        self.wake(hub).notify_one();
    }

    /// The device left `hub`'s network: every scope of it held here goes,
    /// and its follower with it.
    pub(crate) async fn left(&self, hub: &HubId) -> Result<(), String> {
        self.links.lock().expect("links").remove(hub);
        self.unreached.lock().expect("unreached").retain(|(of, _), _| of != hub);
        self.reported.lock().expect("reported").remove(hub);
        if let Some(wake) = self.wakes.lock().expect("wakes").remove(hub) {
            wake.notify_one();
        }
        let (theirs, device) = (hub.as_str().to_string(), self.device());
        blocking(&self.library, move |library| library.leave(&theirs, &device)).await?;
        self.upkeep().await
    }

    /// Leaves each network the library is on that is not among `hubs`, the
    /// device's now: one it left while the copy was not kept, or as it
    /// stopped. Call on a thread that may block.
    pub(crate) fn only_on(&self, hubs: &BTreeSet<String>) -> io::Result<()> {
        let error = |error: fastctx_content::Error| io::Error::other(error.to_string());
        for hub in self.library.joined().map_err(error)? {
            if !hubs.contains(&hub) {
                self.library.leave(&hub, &self.device).map_err(error)?;
            }
        }
        Ok(())
    }

    fn link(&self, hub: &HubId) -> Result<Arc<Link>, String> {
        self.links.lock().expect("links").get(hub).and_then(Weak::upgrade).ok_or_else(|| format!("no link to {hub}"))
    }

    /// Takes in the page of `stream` of `scope` below `before` (the top when
    /// none), as it is read (§29 3.4).
    pub async fn older(
        self: &Arc<Self>,
        hub: &HubId,
        scope: &str,
        stream: &str,
        before: Option<(Vec<u8>, Vec<u8>)>,
        limit: u32,
    ) -> Result<OlderTaken, String> {
        let link = self.link(hub)?;
        let taken = pass::older(self, &link, scope, stream, before, limit).await?;
        // What the page refers to comes after it, as far as the tier
        // fetches, the page read meanwhile (codex-f92d833e phone #6).
        let (replica, hub) = (self.clone(), hub.clone());
        tokio::spawn(async move {
            let _ = objects::fetch(&replica, &hub, &link).await;
        });
        Ok(taken)
    }

    /// Fetches `object` of `scope`, as it is read.
    pub async fn fetch(&self, hub: &HubId, scope: &str, object: fastctx_content::ObjectRef) -> Result<(), String> {
        let link = self.link(hub)?;
        objects::fetch_now(self, hub, &link, scope, object).await
    }
}
