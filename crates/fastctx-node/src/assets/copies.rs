//! Whether this machine goes on taking in its copies of the conversations
//! that do not live here, by account (storage relief, 2026-11-20): on a
//! computer the application is installed on, and wherever it was opened, it
//! does; on a machine without it, a server, it stops, as its owner may say
//! of any. A machine that stopped keeps what it took, holds it under the hub
//! as before, sends up what it writes and gives back what the hub lacks:
//! only what comes down of a conversation living elsewhere stops — its
//! frames, and the objects only it refers to — and what is here of it is
//! not read as its copy, its own machine asked instead. Nothing here ends a
//! holding or deletes anything: the hub is told nothing of this.
//!
//! A scope once paused owes a whole pull: however it comes to be taken in
//! again — its owner's word, the application opened, another reader here
//! taking it in, the conversation coming to live here — it is marked not
//! pulled whole before it is read here again, and pulled by itself from
//! where each of its streams stands here (`settle`). What is owed is kept,
//! so a machine stopped short owes it still.
//!
//! What is kept is by hub, account and when the account was opened, in
//! `copies.json` beside the library: one made again under the same name
//! starts as a new one would.
//!
//! At its owner's word a machine lets go the copies it holds that every
//! reader here paused and that hold nothing of its own (`clear`): their
//! holdings end, their purge takes them, and while every reader here stays
//! paused none is held anew; one of them taking it in again takes it whole
//! from where it lives.

use crate::Node;
use fastctx_content::v2::HeldScope;
use fastctx_types::update::Kind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FILE: &str = "copies.json";

/// The conversations living here are found again once residents changed,
/// no sooner than this after the last time.
const HOSTED_AGAIN: Duration = Duration::from_secs(10);
/// And at the latest this long after, whatever changed.
const HOSTED_AT_MOST: Duration = Duration::from_secs(60);

/// What is paused is worked out again at a pass no sooner than this after
/// the last time; at once as an account's word is set.
const SETTLE_AGAIN: Duration = Duration::from_secs(30);

/// Whether an account's copies are taken in here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    On,
    Paused,
}

/// What set it so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    /// As this machine was found installed: with the application, on;
    /// without, paused.
    Install,
    /// The application was opened here as the account.
    App,
    /// The machine's owner said so.
    Owner,
}

/// One account's word here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) hub: String,
    pub(crate) account: String,
    /// When the account was opened, in milliseconds.
    pub(crate) opened: i64,
    pub(crate) state: State,
    pub(crate) source: Source,
    pub(crate) at: jiff::Timestamp,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    accounts: Vec<Entry>,
    /// The scopes paused here since they were last pulled whole.
    #[serde(default)]
    owed: BTreeSet<String>,
    /// The copies let go here at the owner's word (`clear`), each with who
    /// read it here then, `account@opened`: none is held anew while every
    /// reader here is paused.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    cleared: BTreeMap<String, Vec<String>>,
}

/// What was asked of an account's copies here, and what came of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Set {
    /// It changed, from paused to on or back.
    Changed,
    /// It was so already.
    Unchanged,
    /// Its owner paused it: the application opened does not take it on.
    OwnersWord,
}

/// The scopes of the conversations living here as last found, with the
/// count of resident changes they were found at, and when.
type Hosted = Option<(u64, Instant, Arc<BTreeSet<String>>)>;

pub(crate) struct Copies {
    path: PathBuf,
    kept: Mutex<Kept>,
    /// The file could not be read: nothing is paused, and nothing is
    /// written over it.
    broken: bool,
    hosted: Mutex<Hosted>,
    /// What is taken in here no more, as last settled; what was owed as the
    /// node starts, before that.
    paused: Mutex<Arc<BTreeSet<String>>>,
    /// One settling at a time, and when the last one was.
    settling: Mutex<Option<Instant>>,
    /// Held while copies are let go (`clear`), an account is taken in again
    /// (`set_here`) or a listing holds anew what was let go (`hold_listed`):
    /// none of these goes between another's looking and doing.
    clearing: Mutex<()>,
    /// The scopes taken in again since they were last pulled whole, each
    /// with the count of takings-in it was at: read here as no copy until a
    /// pull begun after that marks it pulled whole (`pulled_since`).
    released: Mutex<BTreeMap<String, u64>>,
    releases: std::sync::atomic::AtomicU64,
}

impl Copies {
    /// What is kept in `dir`, the asset library's.
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let (kept, broken) = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Kept>(&bytes) {
                Ok(kept) => (kept, false),
                Err(_) => (Kept::default(), true),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Kept::default(), false),
            Err(_) => (Kept::default(), true),
        };
        let paused = Arc::new(kept.owed.clone());
        Self {
            path,
            kept: Mutex::new(kept),
            broken,
            hosted: Mutex::new(None),
            paused: Mutex::new(paused),
            settling: Mutex::new(None),
            clearing: Mutex::new(()),
            released: Mutex::new(BTreeMap::new()),
            releases: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Where `scope` stands in being taken in again: what a pull notes as it
    /// begins, and gives back as it ends (`pulled_since`).
    pub(crate) fn release_mark(&self, scope: &str) -> Option<u64> {
        self.released.lock().expect("released").get(scope).copied()
    }

    /// Whether a pull of `scope` begun at `mark` may mark it pulled whole: it
    /// was taken in again no later than that. If so, it is no longer kept as
    /// taken in again.
    pub(crate) fn pulled_since(&self, scope: &str, mark: Option<u64>) -> bool {
        let mut released = self.released.lock().expect("released");
        if released.get(scope).copied() != mark {
            return false;
        }
        released.remove(scope);
        true
    }

    /// Marks `scope` pulled whole in `holding` (`Library::pulled`), its pull
    /// begun at `mark`, unless it was taken in again since: judged and marked
    /// under the lock taking in again holds, so neither goes between the
    /// other's looking and doing. Whether it was marked.
    pub(crate) fn mark_pulled(
        &self,
        library: &fastctx_content::v2::Library,
        scope: &str,
        holding: i64,
        mark: Option<u64>,
    ) -> Result<bool, String> {
        let _settling = self.settling.lock().expect("settling");
        if self.release_mark(scope) != mark {
            return Ok(false);
        }
        library.pulled(scope, holding).map_err(|error| error.to_string())?;
        self.pulled_since(scope, mark);
        Ok(true)
    }

    fn mark_released(&self, scope: &str) {
        let mark = self.releases.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.released.lock().expect("released").insert(scope.to_string(), mark);
    }

    fn is_released(&self, scope: &str) -> bool {
        self.released.lock().expect("released").contains_key(scope)
    }

    /// Every account's word here.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        self.kept.lock().expect("copies").accounts.clone()
    }

    /// What is taken in here no more, as last settled: looked up, nothing
    /// read, on whatever thread.
    pub(crate) fn paused_now(&self) -> Arc<BTreeSet<String>> {
        self.paused.lock().expect("paused").clone()
    }

    /// Whether the account `account@opened` of `hub` takes in its copies
    /// here. None kept yet, it is kept as this machine is installed, once
    /// that is known (`kind`): on until then. Not kept for a write that
    /// failed, it is worked out the same way again.
    pub(crate) fn state(&self, hub: &str, account: &str, opened: i64, kind: Option<Kind>) -> State {
        if self.broken {
            return State::On;
        }
        let mut kept = self.kept.lock().expect("copies");
        if let Some(entry) =
            kept.accounts.iter().find(|entry| entry.hub == hub && entry.account == account && entry.opened == opened)
        {
            return entry.state;
        }
        let Some(kind) = kind else { return State::On };
        let state = if kind == Kind::Headless { State::Paused } else { State::On };
        let mut next = kept.clone();
        next.accounts.push(Entry {
            hub: hub.to_string(),
            account: account.to_string(),
            opened,
            state,
            source: Source::Install,
            at: jiff::Timestamp::now(),
        });
        if self.write(&next).is_ok() {
            *kept = next;
        }
        state
    }

    /// Sets whether `account@opened` of `hub` takes in its copies here, as
    /// `source` says: the application opened takes on no account its owner
    /// paused; the owner's word is kept as the owner's, though it says what
    /// was so already. Kept on disk before it holds.
    pub(crate) fn set(
        &self,
        hub: &str,
        account: &str,
        opened: i64,
        state: State,
        source: Source,
    ) -> std::io::Result<Set> {
        if self.broken {
            return Err(std::io::Error::other(format!("{} could not be read", self.path.display())));
        }
        let mut kept = self.kept.lock().expect("copies");
        let found = kept
            .accounts
            .iter()
            .position(|entry| entry.hub == hub && entry.account == account && entry.opened == opened);
        let was = found.map(|at| kept.accounts[at].clone());
        if let Some(was) = &was {
            if source == Source::App && was.state == State::Paused && was.source == Source::Owner {
                return Ok(Set::OwnersWord);
            }
            if was.state == state && (source != Source::Owner || was.source == Source::Owner) {
                return Ok(Set::Unchanged);
            }
        }
        let entry = Entry {
            hub: hub.to_string(),
            account: account.to_string(),
            opened,
            state,
            source,
            at: jiff::Timestamp::now(),
        };
        let mut next = kept.clone();
        match found {
            Some(at) => next.accounts[at] = entry,
            None => next.accounts.push(entry),
        }
        self.write(&next)?;
        *kept = next;
        Ok(if was.is_some_and(|was| was.state == state) { Set::Unchanged } else { Set::Changed })
    }

    /// Whether the copy of `scope` was let go here at the owner's word.
    pub(crate) fn is_cleared(&self, scope: &str) -> bool {
        self.kept.lock().expect("copies").cleared.contains_key(scope)
    }

    /// Keeps that the copy of `scope`, read here by `readers`, is let go,
    /// or no longer (`readers` none); on disk before it holds.
    fn mark_cleared(&self, scope: &str, readers: Option<Vec<String>>) -> std::io::Result<()> {
        if self.broken {
            return Err(std::io::Error::other(format!("{} could not be read", self.path.display())));
        }
        let mut kept = self.kept.lock().expect("copies");
        let mut next = kept.clone();
        match readers {
            Some(readers) => {
                next.cleared.insert(scope.to_string(), readers);
            }
            None => {
                if next.cleared.remove(scope).is_none() {
                    return Ok(());
                }
            }
        }
        self.write(&next)?;
        *kept = next;
        Ok(())
    }

    /// What was found living here is found anew at the next look.
    fn forget_hosted(&self) {
        *self.hosted.lock().expect("hosted") = None;
    }

    fn write(&self, kept: &Kept) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(kept).map_err(std::io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, fastctx_platform::Permissions::Private)
    }

    /// The scopes of the conversations living on this machine — asleep or
    /// not, moved here or not taken over yet — as last found.
    fn hosted(&self, node: &Node) -> Arc<BTreeSet<String>> {
        let at = *node.residents.changes().borrow();
        let mut found = self.hosted.lock().expect("hosted");
        if let Some((was, when, scopes)) = &*found {
            let stale = when.elapsed() >= HOSTED_AT_MOST || (*was != at && when.elapsed() >= HOSTED_AGAIN);
            if !stale {
                return scopes.clone();
            }
        }
        let mut scopes = BTreeSet::new();
        for record in node.residents.everyone() {
            if record.removed.is_some() {
                continue;
            }
            if let Ok(Some((_, scope))) = crate::timeline::scope_of(node, &record.id) {
                scopes.insert(scope);
            }
        }
        let scopes = Arc::new(scopes);
        *found = Some((at, Instant::now(), scopes.clone()));
        scopes
    }

    /// The scopes of the conversations on this machine found now and whole,
    /// one removed and not gone yet among them; an error where that of any
    /// cannot be told.
    fn hosted_now(&self, node: &Node) -> Result<BTreeSet<String>, String> {
        let mut scopes = BTreeSet::new();
        for record in node.residents.everyone() {
            match crate::timeline::scope_of(node, &record.id) {
                Ok(Some((_, scope))) => {
                    scopes.insert(scope);
                }
                Ok(None) => {}
                Err(failure) => return Err(format!("finding which conversations live here: {}", failure.message)),
            }
        }
        Ok(scopes)
    }
}

/// Who the hub last listed as reading a scope held here, as the driver
/// keeps it (`session::discover`): `{"of": ["account@opened", …], …}`.
#[derive(Deserialize)]
struct Listed {
    #[serde(default)]
    of: Vec<String>,
}

/// The accounts a scope's readers here are, as the hub last listed them,
/// each with when it was opened; none where that cannot be read.
fn readers(scope: &HeldScope) -> Vec<(String, i64)> {
    let Some(listed) = scope.readers.as_deref().and_then(|text| serde_json::from_str::<Listed>(text).ok()) else {
        return Vec::new();
    };
    listed
        .of
        .iter()
        .filter_map(|one| {
            let (account, opened) = one.rsplit_once('@')?;
            Some((account.to_string(), opened.parse().ok()?))
        })
        .collect()
}

/// Whether `account@opened` is among the readers of `scope` here.
fn read_by(scope: &HeldScope, account: &str, opened: i64) -> bool {
    readers(scope).iter().any(|(one, at)| one == account && *at == opened)
}

/// The readers of `scope` the hub listed that are logged in here now, each
/// the account opened when it was listed.
fn readers_here(node: &Node, scope: &HeldScope) -> Vec<(String, i64)> {
    let Some(link) = node.links().into_iter().find(|link| Some(link.membership().id.as_str()) == scope.hub.as_deref())
    else {
        return Vec::new();
    };
    let view = link.view();
    let logins: std::collections::BTreeMap<String, i64> = node
        .accounts_on(&link, &fastctx_types::access::Acting::Here)
        .into_iter()
        .filter_map(|account| {
            let opened = view.directory.account(&account)?.opened.as_millisecond();
            Some((account.to_string(), opened))
        })
        .collect();
    readers(scope).into_iter().filter(|(account, opened)| logins.get(account) == Some(opened)).collect()
}

/// Whether nothing of `scope` is to be taken in here now: a conversation's,
/// every reader of which logged in here paused it, which does not live here
/// and is not being taken over here. `only`, where given, is the one
/// account it is read as: whether that one paused it decides. Where
/// anything of it cannot be told, it is taken in. Call on a thread that may
/// block: it reads the library, and may keep a first word.
fn paused(node: &Node, scope: &HeldScope, only: Option<&fastctx_types::AccountName>) -> bool {
    let Some(assets) = node.assets.as_ref() else { return false };
    // A conversation's alone, a machine's or a body's: a body's log and
    // records, the hub's log and what an account keeps are taken in as
    // before.
    if !fastctx_types::assets::of_conversation(&scope.key) {
        return false;
    }
    let Some(hub) = scope.hub.as_deref() else { return false };
    let mut readers = readers_here(node, scope);
    if let Some(only) = only {
        readers.retain(|(account, _)| account == only.as_str());
    }
    if readers.is_empty() {
        return false;
    }
    let kind = crate::updates::install_kind(node);
    if !readers.iter().all(|(account, opened)| assets.copies.state(hub, account, *opened, kind) == State::Paused) {
        return false;
    }
    if assets.copies.hosted(node).contains(&scope.key)
        || assets.adopting.lock().expect("adopting").contains(&scope.key)
        || assets.takeovers.lock().expect("takeovers").iter().any(|(taken, _)| *taken == scope.key)
    {
        return false;
    }
    // A stream this machine orders in it is made here: it lives here, or
    // did and is handing it over still.
    assets.library.own_streams(&scope.key, &assets.device()).is_ok_and(|own| own.is_empty())
}

/// Works out again what is taken in here no more, and keeps what that owes:
/// a scope paused now owes a whole pull; one owing it that is taken in again
/// is marked not pulled whole, then read here no more as a copy until it is
/// pulled whole again, each stream from where it stands here; one not held
/// any more owes nothing. What is owed is kept on disk before what is
/// paused is published: where it cannot be, what was published stays, and
/// why is said. The scopes paused, as published. Call on a thread that may
/// block.
pub(crate) fn settle(node: &Arc<Node>, now_rather: bool) -> Result<Arc<BTreeSet<String>>, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(Default::default()) };
    let mut last = assets.copies.settling.lock().expect("settling");
    if !now_rather && last.is_some_and(|at| at.elapsed() < SETTLE_AGAIN) {
        return Ok(assets.copies.paused_now());
    }
    *last = Some(Instant::now());
    let held = assets.library.held_scopes().map_err(|error| format!("listing the scopes held here: {error}"))?;
    let held_keys: BTreeSet<&str> = held.iter().map(|scope| scope.key.as_str()).collect();
    let mut now: BTreeSet<String> =
        held.iter().filter(|scope| paused(node, scope, None)).map(|scope| scope.key.clone()).collect();
    let owed = assets.copies.kept.lock().expect("copies").owed.clone();
    let mut released = BTreeSet::new();
    // Those that could not be marked stay paused, and are said.
    let mut unmarked: Option<(usize, String)> = None;
    for key in &owed {
        if !held_keys.contains(key.as_str()) || now.contains(key) {
            continue;
        }
        // Taken in again: not read here before it is pulled whole again, by
        // a pull begun from now on.
        match assets.library.forget_pulled_of(key) {
            Ok(()) => {
                assets.copies.mark_released(key);
                released.insert(key.clone());
            }
            Err(error) => {
                now.insert(key.clone());
                let (count, _) = unmarked.get_or_insert_with(|| (0, error.to_string()));
                *count += 1;
            }
        }
    }
    let still: BTreeSet<String> = owed.iter().filter(|key| held_keys.contains(key.as_str())).cloned().collect();
    let next_owed: BTreeSet<String> =
        still.into_iter().filter(|key| !released.contains(key)).chain(now.iter().cloned()).collect();
    if next_owed != owed && !assets.copies.broken {
        let mut kept = assets.copies.kept.lock().expect("copies");
        let mut next = kept.clone();
        next.owed = next_owed;
        // Not kept, nothing newly paused is published: a scope passed by
        // while what it owes was not kept would be read whole after.
        assets.copies.write(&next).map_err(|error| format!("keeping what the copies here owe: {error}"))?;
        *kept = next;
    }
    let now = Arc::new(now);
    // Swapped alone under the lock; compared, and the last let go, outside.
    let was = std::mem::replace(&mut *assets.copies.paused.lock().expect("paused"), now.clone());
    let changed = *was != *now;
    drop(was);
    if !released.is_empty() {
        let mut relook = assets.relook.lock().expect("relook");
        relook.extend(released.iter().cloned());
    }
    if changed {
        assets.metadata_moved.send_modify(|count| *count = count.wrapping_add(1));
    }
    if !released.is_empty() {
        for link in node.links() {
            super::session::wake(node, link.hub());
        }
    }
    if let Some((count, first)) = unmarked {
        return Err(format!(
            "{count} conversations stay paused, as they could not be marked to be pulled again: {first}"
        ));
    }
    Ok(now)
}

/// Whether nothing of `scope` is taken in here now, for a reader on
/// whatever thread it runs: what was last settled is looked up, and
/// nothing is read from the disk or the library.
pub(crate) fn paused_here(node: &Node, scope: &HeldScope) -> bool {
    // Taken in again and not pulled whole since: what was looked up of it
    // before it was taken in may say pulled whole still.
    node.assets
        .as_ref()
        .is_some_and(|assets| assets.copies.paused_now().contains(&scope.key) || assets.copies.is_released(&scope.key))
}

/// Whether the copy of `scope`, listed by the hub of `hub` as read here by
/// `readers` (`account@opened`), stays let go: it was let go here at the
/// owner's word and every reader of it logged in here is paused. One of them
/// taking it in again, it is no longer kept as let go, and is held anew.
/// Call on a thread that may block.
pub(crate) fn stays_cleared(node: &Node, hub: &str, scope: &str, readers: &[String]) -> bool {
    let Some(assets) = node.assets.as_ref() else { return false };
    if !assets.copies.is_cleared(scope) {
        return false;
    }
    // Living here now, or being taken in to live here: held anew, whoever
    // reads it.
    let taken = lives_here(node, scope) || {
        let Some(link) = node.links().into_iter().find(|link| link.hub().as_str() == hub) else { return true };
        let here: BTreeMap<String, i64> = {
            let view = link.view();
            node.accounts_on(&link, &fastctx_types::access::Acting::Here)
                .into_iter()
                .filter_map(|account| {
                    Some((account.to_string(), view.directory.account(&account)?.opened.as_millisecond()))
                })
                .collect()
        };
        let kind = crate::updates::install_kind(node);
        readers.iter().filter_map(|one| one.rsplit_once('@')).any(|(account, opened)| {
            opened.parse::<i64>().is_ok_and(|opened| {
                here.get(account) == Some(&opened) && assets.copies.state(hub, account, opened, kind) == State::On
            })
        })
    };
    if taken && assets.copies.mark_cleared(scope, None).is_ok() {
        return false;
    }
    true
}

/// Installs `scope` as the hub of `hub` listed it, `listed` (its network,
/// descriptor and revision), with what this machine may hold of it and who
/// logged in here reads it, `kept` (`Library::allow`: the holds and the
/// readers as the driver keeps them) — but where its copy
/// stays let go here (`stays_cleared`), this machine began it and describes
/// it itself (`session::describe`, which judges who it was begun for), or
/// its conversation is deleted here, or being. Looked at and done under the
/// lock a clearing holds, so none lets it go between the two, nor judges it
/// by readers that changed meanwhile. Whether it is held so.
pub(crate) fn hold_listed(
    node: &Node,
    hub: &str,
    scope: &str,
    readers: &[String],
    (network, description, revision): (&fastctx_content::v2::Network, Vec<u8>, i64),
    (holds, said): (&str, &str),
) -> Result<bool, String> {
    let assets = node.assets.as_ref().ok_or("the asset library is unavailable")?;
    let _clearing = assets.copies.clearing.lock().expect("clearing");
    if stays_cleared(node, hub, scope, readers) {
        return Ok(false);
    }
    // A conversation of this machine's deleted here, or being: not held anew.
    if let Some(id) = conversation_of(scope)
        && born_here(node, scope)
        && (node.residents.is_deleting(&id) || node.residents.deleted(&id).is_some())
    {
        return Ok(false);
    }
    let here = assets.library.held_listing(scope).map_err(|error| error.to_string())?;
    if here.as_ref().map(|listing| listing.description.as_slice()) != Some(description.as_slice()) {
        if assets
            .library
            .install_listed(network, scope, description, revision)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(false);
        }
        // Held again: a timeline that waited for it is brought now
        // (`timeline::tail::Produced::waits_listing`).
        if here.is_none()
            && let Some(id) = conversation_of(scope)
        {
            crate::timeline::described(node, &id);
        }
    }
    let listed = here.as_ref().map(|listing| (listing.allowed.as_deref(), listing.readers.as_deref()));
    if listed != Some((Some(holds), Some(said))) {
        assets.library.allow(scope, holds, said).map_err(|error| error.to_string())?;
    }
    Ok(true)
}

/// Whether `scope` may be of a conversation born on this machine: under its
/// name now or one it had on that network (`c/hub/machine/id`), or one born
/// on no machine named (`c/hub/-/account/id`), whose account and id another
/// machine's meets no more than it meets this one's.
fn born_here(node: &Node, scope: &str) -> bool {
    let mut parts = scope.split('/');
    let (Some("c"), Some(hub), Some(machine)) = (parts.next(), parts.next(), parts.next()) else { return false };
    if machine == "-" || node.machine().as_str() == machine {
        return true;
    }
    node.links().into_iter().filter(|link| link.hub().as_str() == hub).any(|link| {
        let me = link.membership().name.clone();
        me.as_str() == machine
            || link
                .view()
                .directory
                .member(&me)
                .is_some_and(|member| member.was.iter().any(|was| was.as_str() == machine))
    })
}

/// The conversation a conversation's scope is of, by the end of its key.
fn conversation_of(scope: &str) -> Option<fastctx_types::ResidentId> {
    if !fastctx_types::assets::of_conversation(scope) {
        return None;
    }
    fastctx_types::ResidentId::try_from(scope.rsplit('/').next()?).ok()
}

/// Whether the conversation of `scope` lives here now, or is being taken in
/// to live here: as found now, not as last found.
fn lives_here(node: &Node, scope: &str) -> bool {
    let Some(assets) = node.assets.as_ref() else { return false };
    conversation_of(scope).is_some_and(|id| node.residents.about(&id).is_some())
        || assets.adopting.lock().expect("adopting").contains(scope)
        || assets.takeovers.lock().expect("takeovers").iter().any(|(taken, _)| taken == scope)
}

/// Lets go, as the machine's owner asked, the copies here `account` of
/// `hub` reads that every reader here paused and that live elsewhere —
/// found anew and whole, not as last found — or, without `apply`, counts
/// them, judged as letting go judges each (`Library::copy_kept`); `only`,
/// where given, those of them a count said. Each is kept as let go before
/// it is, so no pass holds it anew while its readers here stay paused, and
/// is let go in the library only where nothing of it is this machine's
/// (`Library::drop_copy`), kept otherwise and said. Stopped short after some
/// went, what went is said with why it stopped. What is let go goes with
/// the purge. Call on a thread that may block.
pub(crate) fn clear(
    node: &Arc<Node>,
    hub: &str,
    account: &str,
    apply: bool,
    only: Option<&BTreeSet<(String, i64)>>,
    still: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<fastctx_types::data::Cleared, String> {
    let assets = node.assets.as_ref().ok_or("the asset library is unavailable")?;
    let link = node
        .links()
        .into_iter()
        .find(|link| link.hub().as_str() == hub)
        .ok_or_else(|| format!("this machine is on no network of the hub {hub}"))?;
    let name = fastctx_types::AccountName::try_from(account.to_string()).map_err(|error| error.to_string())?;
    let opened = link
        .view()
        .directory
        .account(&name)
        .map(|found| found.opened.as_millisecond())
        .ok_or_else(|| format!("there is no account {account} on {hub}"))?;
    let mut cleared = fastctx_types::data::Cleared {
        applied: apply,
        scopes: 0,
        bytes: 0,
        kept: Vec::new(),
        keys: Vec::new(),
        failed: None,
    };
    {
        // One at a time; none while what is paused is worked out, an account
        // is taken in again or a listing holds anew what was let go.
        let _clearing = assets.copies.clearing.lock().expect("clearing");
        let _settling = assets.copies.settling.lock().expect("settling");
        // Whoever asked from afar may ask still, as it waited its turn.
        if let Some(still) = still {
            still()?;
        }
        assets.copies.forget_hosted();
        let living = assets.copies.hosted_now(node)?;
        let device = assets.device();
        let held = assets.library.held_scopes().map_err(|error| format!("listing the scopes held here: {error}"))?;
        let mut chosen = Vec::new();
        for scope in held {
            if scope.hub.as_deref() != Some(hub)
                || only.is_some_and(|only| !only.contains(&(scope.key.clone(), scope.holding)))
                || !read_by(&scope, account, opened)
                || living.contains(&scope.key)
                || !paused(node, &scope, None)
            {
                continue;
            }
            match assets.library.copy_kept(&scope.key, scope.holding, &device).map_err(|error| error.to_string())? {
                Some(why) => cleared.kept.push((scope.key.clone(), why)),
                None => chosen.push(scope),
            }
        }
        let held =
            |scope: &HeldScope| fastctx_types::data::CopyHeld { scope: scope.key.clone(), holding: scope.holding };
        // The room letting them go gives back: their frames, and the objects
        // nothing else here refers to, holds or leases.
        let freed = |size: fastctx_content::v2::CopySize| size.frames + size.objects;
        if !apply {
            let keys: Vec<String> = chosen.iter().map(|scope| scope.key.clone()).collect();
            let all = assets.library.copies_freed(&keys).map_err(|error| error.to_string())?;
            cleared.scopes = keys.len() as u64;
            cleared.bytes = freed(all);
            cleared.keys = chosen.iter().map(held).collect();
            return Ok(cleared);
        }
        // Judged again before any goes: one whose conversation came to live
        // here, or that a reader here took in again meanwhile, stays.
        let mut going = Vec::new();
        for scope in chosen {
            if lives_here(node, &scope.key) {
                cleared.kept.push((scope.key.clone(), "its conversation lives here now".into()));
            } else if !paused(node, &scope, None) {
                cleared.kept.push((scope.key.clone(), "a reader here takes it in now".into()));
            } else {
                going.push(scope);
            }
        }
        let keys: Vec<String> = going.iter().map(|scope| scope.key.clone()).collect();
        let all = assets.library.copies_freed(&keys).map_err(|error| error.to_string())?;
        let mut gone = Vec::new();
        for scope in going {
            // And once more just as it goes: one whose conversation came to
            // live here while those before it went, or that a reader took in
            // again, stays.
            if lives_here(node, &scope.key) {
                cleared.kept.push((scope.key.clone(), "its conversation lives here now".into()));
                continue;
            }
            if !paused(node, &scope, None) {
                cleared.kept.push((scope.key.clone(), "a reader here takes it in now".into()));
                continue;
            }
            let readers: Vec<String> = readers(&scope).into_iter().map(|(one, at)| format!("{one}@{at}")).collect();
            // Failed, whether it stays kept as let go though it was not.
            let step = (|| -> Result<Option<String>, (String, bool)> {
                assets
                    .copies
                    .mark_cleared(&scope.key, Some(readers))
                    .map_err(|error| (format!("keeping what is let go: {error}"), false))?;
                let unmark = |why: String| match assets.copies.mark_cleared(&scope.key, None) {
                    Ok(()) => (why, false),
                    Err(error) => {
                        (format!("{why}; and it stays kept as let go here, though it was not: {error}"), true)
                    }
                };
                match assets.library.drop_copy(&scope.key, scope.holding, &device) {
                    Ok(fastctx_content::v2::Dropped::Gone) => Ok(None),
                    Ok(fastctx_content::v2::Dropped::Kept(why)) => match assets.copies.mark_cleared(&scope.key, None) {
                        Ok(()) => Ok(Some(why)),
                        Err(error) => Err((format!("keeping that {} was not let go: {error}", scope.key), true)),
                    },
                    Err(error) => Err(unmark(format!("letting go {}: {error}", scope.key))),
                }
            })();
            match step {
                Ok(None) => gone.push(held(&scope)),
                Ok(Some(why)) => cleared.kept.push((scope.key.clone(), why)),
                Err((why, false)) if gone.is_empty() => return Err(why),
                Err((why, _)) => {
                    cleared.failed = Some(why);
                    break;
                }
            }
        }
        // All of them gone, what they gave back together; some, not told —
        // what they shared with those kept stays — and said so.
        if gone.len() == keys.len() {
            cleared.bytes = freed(all);
        } else if !gone.is_empty() && cleared.failed.is_none() {
            cleared.failed = Some("some were kept as they were let go: the room given back is told once all go".into());
        }
        cleared.scopes = gone.len() as u64;
        cleared.keys = gone;
    }
    if cleared.scopes > 0 {
        if let Err(error) = settle(node, true) {
            cleared.failed.get_or_insert(format!("let go, but what follows is not settled yet: {error}"));
        }
        super::upkeep::wake(node);
    }
    Ok(cleared)
}

/// What the machine's owner asks of the copies here (`RemoteCall::Copies`,
/// or the user here by `Call::MachineCopies`): asked over the network of
/// `through`, only that network's copies are answered for or changed — its
/// hub vouched for the asker, no other did. Call on a thread that may block.
pub(crate) fn asked(
    node: &Arc<Node>,
    ask: fastctx_types::data::CopiesAsk,
    through: Option<(&fastctx_types::access::HubId, &fastctx_types::AccountName)>,
) -> Result<fastctx_types::data::CopiesAnswer, String> {
    use fastctx_types::data::{CopiesAnswer, CopiesAsk};
    let within = |hub: &str| match through {
        Some((through, _)) if through.as_str() != hub => {
            Err(format!("asked over the network of {}, only its copies are managed from there", through.as_str()))
        }
        _ => Ok(()),
    };
    // Asked from afar, as the machine's owner: so still as it is done.
    let still = || match through {
        Some((hub, caller)) => owned_by(node, hub, caller),
        None => Ok(()),
    };
    let reported = || {
        let mut accounts = report(node);
        if let Some((through, _)) = through {
            accounts.retain(|one| one.hub == through.as_str());
        }
        accounts
    };
    match ask {
        CopiesAsk::Report => Ok(CopiesAnswer::Report { accounts: reported() }),
        CopiesAsk::Set { hub, account, on } => {
            within(&hub)?;
            let state = if on { State::On } else { State::Paused };
            set_here_as(node, &hub, &account, None, state, Source::Owner, Some(&still))?;
            Ok(CopiesAnswer::Report { accounts: reported() })
        }
        CopiesAsk::Clear { hub, account, apply, only } => {
            within(&hub)?;
            let only: Option<BTreeSet<(String, i64)>> =
                only.map(|held| held.into_iter().map(|one| (one.scope, one.holding)).collect());
            clear(node, &hub, &account, apply, only.as_ref(), Some(&still)).map(CopiesAnswer::Cleared)
        }
    }
}

/// Whether this machine is `caller`'s on the network of `hub`, as its
/// directory says now, and why not: a view from before the link was last
/// made again is no word on it.
fn owned_by(
    node: &Node,
    hub: &fastctx_types::access::HubId,
    caller: &fastctx_types::AccountName,
) -> Result<(), String> {
    let Some(link) = node.links().into_iter().find(|link| link.hub() == hub) else {
        return Err("this machine is not on that network now".into());
    };
    if !link.view_current() {
        return Err("this machine is connecting to its hub again: ask again in a moment".into());
    }
    let me = link.membership().name.clone();
    if link.view().directory.member(&me).is_some_and(|member| member.home.as_ref() == Some(caller)) {
        Ok(())
    } else {
        Err(format!("this machine is no longer {caller}'s"))
    }
}

/// The same of `machine`, this one or another of the network's: there,
/// only its owner is answered (`admission`).
pub(crate) async fn machine(
    node: &Arc<Node>,
    machine: fastctx_types::access::Written,
    ask: fastctx_types::data::CopiesAsk,
) -> Result<fastctx_types::data::CopiesAnswer, fastctx_types::ipc::Failure> {
    use fastctx_types::ipc::Failure;
    if node.is_here(&machine) {
        let asking = node.clone();
        return fastctx_platform::task::spawn_blocking(move || asked(&asking, ask, None))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run);
    }
    let call = fastctx_types::remote::RemoteCall::Copies { ask };
    match node
        .call(&fastctx_types::access::Acting::Here, &machine, fastctx_types::access::Purpose::Hands, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        fastctx_types::remote::RemoteReply::Copies(answer) => Ok(answer),
        _ => Err(Failure::new(format!("{machine} answered something other than its copies"))),
    }
}

/// Each account logged in here, on each hub: whether it takes in its copies,
/// and how many of the scopes it reads are taken in here no more. Call on a
/// thread that may block.
pub(crate) fn report(node: &Node) -> Vec<fastctx_types::data::CopiesOf> {
    let Some(assets) = node.assets.as_ref() else { return Vec::new() };
    let kind = crate::updates::install_kind(node);
    let held = assets.library.held_scopes().unwrap_or_default();
    let paused = assets.copies.paused_now();
    let mut found = Vec::new();
    for link in node.links() {
        let hub = link.hub().as_str().to_string();
        let view = link.view();
        let Some(me) = view.me() else { continue };
        for account in &me.logins {
            let Some(opened) = view.directory.account(account).map(|found| found.opened.as_millisecond()) else {
                continue;
            };
            let state = assets.copies.state(&hub, account.as_str(), opened, kind);
            let entry = assets
                .copies
                .entries()
                .into_iter()
                .find(|entry| entry.hub == hub && entry.account == account.as_str() && entry.opened == opened);
            let paused_here: Vec<&HeldScope> = held
                .iter()
                .filter(|scope| scope.hub.as_deref() == Some(hub.as_str()) && paused.contains(&scope.key))
                .filter(|scope| read_by(scope, account.as_str(), opened))
                .collect();
            let paused_scopes = paused_here.len();
            let keys: Vec<String> = paused_here.iter().map(|scope| scope.key.clone()).collect();
            let paused_bytes = assets.library.copies_size(&keys).map_or(0, |size| size.frames + size.objects);
            found.push(fastctx_types::data::CopiesOf {
                hub: hub.clone(),
                account: account.to_string(),
                opened,
                on: state == State::On,
                source: entry.as_ref().map(|entry| {
                    match entry.source {
                        Source::Install => "install",
                        Source::App => "app",
                        Source::Owner => "owner",
                    }
                    .to_string()
                }),
                at: entry.map(|entry| entry.at),
                paused_scopes: paused_scopes as u64,
                paused_bytes,
            });
        }
    }
    found
}

/// Sets whether `account` of the hub `hub`, logged in here, takes in its
/// copies here, as `source` says (`Copies::set`), and settles what follows
/// before it answers: paused, nothing more of them comes down from now on,
/// but for what is under way already; taken in again, they are pulled whole
/// before they are read here. `opened`, where given, is when the account the
/// asker means was opened: one made again since is not it. Call on a thread
/// that may block.
pub(crate) fn set_here(
    node: &Arc<Node>,
    hub: &str,
    account: &str,
    opened: Option<i64>,
    state: State,
    source: Source,
) -> Result<Set, String> {
    set_here_as(node, hub, account, opened, state, source, None)
}

/// `set_here`, whoever asked from afar asked still as its turn comes
/// (`still`), under the lock a clearing holds.
fn set_here_as(
    node: &Arc<Node>,
    hub: &str,
    account: &str,
    opened: Option<i64>,
    state: State,
    source: Source,
    still: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Set, String> {
    let Some(assets) = node.assets.as_ref() else { return Err("the asset library is unavailable".into()) };
    let link = node
        .links()
        .into_iter()
        .find(|link| link.hub().as_str() == hub)
        .ok_or_else(|| format!("this machine is on no network of the hub {hub}"))?;
    let name = fastctx_types::AccountName::try_from(account.to_string()).map_err(|error| error.to_string())?;
    let now = {
        let view = link.view();
        if !view.me().is_some_and(|me| me.logins.contains(&name)) {
            return Err(format!("{account} is not logged in here on {hub}"));
        }
        view.directory
            .account(&name)
            .map(|found| found.opened.as_millisecond())
            .ok_or_else(|| format!("there is no account {account} on {hub}"))?
    };
    if opened.is_some_and(|opened| opened != now) {
        return Err(format!("{account} on {hub} was made again since"));
    }
    let set = {
        // Not while copies are let go: one that found the account paused
        // lets none go after it is taken in again.
        let _clearing = assets.copies.clearing.lock().expect("clearing");
        if let Some(still) = still {
            still()?;
        }
        assets.copies.set(hub, account, now, state, source).map_err(|error| error.to_string())?
    };
    // Kept; what follows is settled again at every pass should it not be now.
    let settled = settle(node, true);
    // Taken in again: what was let go of it is listed again at once, and
    // held anew (`stays_cleared`).
    if state == State::On && set == Set::Changed {
        assets.forget_listing(link.hub());
        super::session::wake(node, link.hub());
    }
    settled.map_err(|error| format!("kept, but what follows is not settled yet: {error}"))?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fastctx-copies-{name}-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
        fastctx_platform::disk::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn kept_as_installed_then_as_said() {
        let dir = dir("said");
        let copies = Copies::open(&dir);
        // Not known yet whether the application is installed: taken in, and
        // nothing kept.
        assert_eq!(copies.state("h", "a", 1, None), State::On);
        assert!(!fastctx_platform::disk::exists(dir.join(FILE)).unwrap());
        // A machine without it pauses; with it, takes in.
        assert_eq!(copies.state("h", "a", 1, Some(Kind::Headless)), State::Paused);
        assert_eq!(copies.state("h", "b", 1, Some(Kind::App)), State::On);
        assert_eq!(copies.state("h", "c", 1, Some(Kind::Managed)), State::On);
        // Kept: found so again, whatever is installed now.
        let copies = Copies::open(&dir);
        assert_eq!(copies.state("h", "a", 1, Some(Kind::App)), State::Paused);
        // The same name opened again is another account.
        assert_eq!(copies.state("h", "a", 2, Some(Kind::App)), State::On);
        // Paused as installed, its owner pausing it makes it the owner's:
        // opening the application no longer takes it on.
        assert_eq!(copies.set("h", "a", 1, State::Paused, Source::Owner).unwrap(), Set::Unchanged);
        assert_eq!(copies.set("h", "a", 1, State::On, Source::App).unwrap(), Set::OwnersWord);
        assert_eq!(copies.state("h", "a", 1, None), State::Paused);
        // Its owner takes it in again; then opened as the application.
        assert_eq!(copies.set("h", "a", 1, State::On, Source::Owner).unwrap(), Set::Changed);
        assert_eq!(copies.set("h", "a", 1, State::On, Source::App).unwrap(), Set::Unchanged);
        assert_eq!(Copies::open(&dir).state("h", "a", 1, None), State::On);
        // Paused as installed, opening the application takes it on.
        assert_eq!(copies.state("h", "d", 1, Some(Kind::Headless)), State::Paused);
        assert_eq!(copies.set("h", "d", 1, State::On, Source::App).unwrap(), Set::Changed);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_owed_is_paused_as_the_node_starts() {
        let dir = dir("owed");
        let kept = Kept {
            accounts: Vec::new(),
            owed: ["c/h/m/r".to_string()].into_iter().collect(),
            cleared: BTreeMap::new(),
        };
        fastctx_platform::disk::write(dir.join(FILE), serde_json::to_vec(&kept).unwrap()).unwrap();
        let copies = Copies::open(&dir);
        assert!(copies.paused_now().contains("c/h/m/r"));
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_not_read_pauses_nothing() {
        let dir = dir("broken");
        fastctx_platform::disk::write(dir.join(FILE), b"not json").unwrap();
        let copies = Copies::open(&dir);
        assert_eq!(copies.state("h", "a", 1, Some(Kind::Headless)), State::On);
        assert!(copies.set("h", "a", 1, State::Paused, Source::Owner).is_err());
        assert!(copies.paused_now().is_empty());
        assert_eq!(fastctx_platform::disk::read(dir.join(FILE)).unwrap(), b"not json");
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    #[test]
    fn readers_as_the_driver_keeps_them() {
        let scope = HeldScope {
            key: "c/h/m/r".into(),
            holding: 1,
            pulled: Some(1),
            allowed: None,
            // As `session::discover` keeps it.
            readers: Some(r#"{"of":["ada@12","bob@34","not one"],"frozen":false}"#.into()),
            hub: Some("h".into()),
            description: None,
        };
        assert_eq!(readers(&scope), vec![("ada".to_string(), 12), ("bob".to_string(), 34)]);
        assert!(read_by(&scope, "bob", 34));
        assert!(!read_by(&scope, "bob", 35));
        // Kept otherwise, read as no one.
        let plain = HeldScope { readers: Some("ada@12".into()), ..scope };
        assert!(readers(&plain).is_empty());
    }
}
