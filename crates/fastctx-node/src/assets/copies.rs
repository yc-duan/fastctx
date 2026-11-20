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
//! What is kept is by hub, account and when the account was opened, in
//! `copies.json` beside the library: one made again under the same name
//! starts as a new one would.

use crate::Node;
use fastctx_content::v2::HeldScope;
use fastctx_types::update::Kind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const FILE: &str = "copies.json";

/// How long the conversations living here are taken as they were found
/// before they are found again, however often residents change.
const HOSTED_AGAIN: std::time::Duration = std::time::Duration::from_secs(10);

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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) hub: String,
    pub(crate) account: String,
    /// When the account was opened, in milliseconds.
    pub(crate) opened: i64,
    pub(crate) state: State,
    pub(crate) source: Source,
    pub(crate) at: jiff::Timestamp,
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    accounts: Vec<Entry>,
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
type Hosted = Option<(u64, std::time::Instant, Arc<BTreeSet<String>>)>;

/// What a pass found paused of each hub's network, at a generation.
type PausedOf = std::collections::HashMap<String, (u64, Arc<BTreeSet<String>>)>;

pub(crate) struct Copies {
    path: PathBuf,
    kept: Mutex<Kept>,
    /// The file could not be read: nothing is paused, and nothing is
    /// written over it.
    broken: bool,
    /// Counted up whenever an account's state changes.
    generation: AtomicU64,
    /// The scopes of the conversations living here, as last found, with the
    /// count of resident changes they were found at and when.
    hosted: Mutex<Hosted>,
    /// The scopes of each hub's network a pass found paused, for what goes
    /// on beside the passes (a push, the objects).
    paused: Mutex<PausedOf>,
}

impl Copies {
    /// What is kept in `dir`, the asset library's.
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let (kept, broken) = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(kept) => (kept, false),
                Err(_) => (Kept::default(), true),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Kept::default(), false),
            Err(_) => (Kept::default(), true),
        };
        Self {
            path,
            kept: Mutex::new(kept),
            broken,
            generation: AtomicU64::new(0),
            hosted: Mutex::new(None),
            paused: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Every account's word here.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        self.kept.lock().expect("copies").accounts.clone()
    }

    /// Whether the account `account@opened` of `hub` takes in its copies
    /// here. None kept yet, it is kept as this machine is installed, once
    /// that is known (`kind`): on until then.
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
        kept.accounts.push(Entry {
            hub: hub.to_string(),
            account: account.to_string(),
            opened,
            state,
            source: Source::Install,
            at: jiff::Timestamp::now(),
        });
        // Not kept, it is worked out the same way again.
        let _ = self.write(&kept);
        state
    }

    /// Sets whether `account@opened` of `hub` takes in its copies here, as
    /// `source` says: the application opened takes on no account its owner
    /// paused.
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
        if let Some(at) = found {
            let entry = &kept.accounts[at];
            if source == Source::App && entry.state == State::Paused && entry.source == Source::Owner {
                return Ok(Set::OwnersWord);
            }
            if entry.state == state {
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
        match found {
            Some(at) => kept.accounts[at] = entry,
            None => kept.accounts.push(entry),
        }
        self.write(&kept)?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        Ok(Set::Changed)
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
        if let Some((was, when, scopes)) = &*found
            && (*was == at || when.elapsed() < HOSTED_AGAIN)
        {
            return scopes.clone();
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
        *found = Some((at, std::time::Instant::now(), scopes.clone()));
        scopes
    }

    /// What a pass found paused of `hub`'s network, for what goes on beside
    /// it: none before a pass found it, or after the states changed.
    pub(crate) fn paused_of(&self, hub: &str) -> Arc<BTreeSet<String>> {
        let generation = self.generation();
        self.paused
            .lock()
            .expect("paused")
            .get(hub)
            .filter(|(at, _)| *at == generation)
            .map_or_else(Default::default, |(_, scopes)| scopes.clone())
    }

    fn found_paused(&self, hub: &str, generation: u64, scopes: Arc<BTreeSet<String>>) {
        self.paused.lock().expect("paused").insert(hub.to_string(), (generation, scopes));
    }
}

/// The accounts a scope's readers here are, as the hub last said: one
/// `account@opened` a line.
fn readers(scope: &HeldScope) -> Vec<(String, i64)> {
    scope
        .readers
        .as_deref()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (account, opened) = line.rsplit_once('@')?;
            Some((account.to_string(), opened.parse().ok()?))
        })
        .collect()
}

/// Whether nothing of `scope` is taken in here now: a conversation's — or
/// the hub's own log — every reader of which logged in here paused it, and
/// which does not live here. `only`, where given, is the one account it is
/// read as: whether that one paused it decides. Where anything of it cannot
/// be told, it is taken in.
pub(crate) fn paused(node: &Node, scope: &HeldScope, only: Option<&fastctx_types::AccountName>) -> bool {
    let Some(assets) = node.assets.as_ref() else { return false };
    if !(scope.key.starts_with("c/") || scope.key.starts_with("h/")) {
        return false;
    }
    let Some(hub) = scope.hub.as_deref() else { return false };
    let mut readers = readers(scope);
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
    if assets.copies.hosted(node).contains(&scope.key) {
        return false;
    }
    // A stream this machine orders in it is made here: it lives here, or
    // did and is handing it over still.
    assets.library.own_streams(&scope.key, &assets.device()).is_ok_and(|own| own.is_empty())
}

/// The scopes of `hub`'s network nothing of which is taken in here now, kept
/// for what goes on beside the passes. Call on a thread that may block.
pub(crate) fn paused_of_hub(node: &Node, hub: &str) -> Arc<BTreeSet<String>> {
    let Some(assets) = node.assets.as_ref() else { return Default::default() };
    let generation = assets.copies.generation();
    let found: BTreeSet<String> = match assets.library.held_scopes() {
        Ok(scopes) => scopes
            .into_iter()
            .filter(|scope| scope.hub.as_deref() == Some(hub) && paused(node, scope, None))
            .map(|scope| scope.key)
            .collect(),
        Err(_) => BTreeSet::new(),
    };
    let found = Arc::new(found);
    assets.copies.found_paused(hub, generation, found.clone());
    found
}

/// Sets whether the account `account@opened` of `hub` takes in its copies
/// here, and what follows: taken in again, each of its scopes is pulled by
/// itself again from where each stream stands here — what passed it by
/// while paused comes down, though nothing of it moves again — and is read
/// here once it is whole; either way the World shown is worked out again.
pub(crate) fn set(
    node: &Arc<Node>,
    hub: &fastctx_types::access::HubId,
    account: &str,
    opened: i64,
    state: State,
    source: Source,
) -> std::io::Result<Set> {
    let Some(assets) = node.assets.as_ref() else {
        return Err(std::io::Error::other("the asset library is unavailable"));
    };
    let set = assets.copies.set(hub.as_str(), account, opened, state, source)?;
    if set != Set::Changed {
        return Ok(set);
    }
    if state == State::On {
        let reader = format!("{account}@{opened}");
        let scopes = assets.library.held_scopes().map_err(|error| std::io::Error::other(error.to_string()))?;
        for scope in scopes {
            let read = scope.readers.as_deref().unwrap_or_default().lines().any(|line| line == reader);
            if scope.hub.as_deref() == Some(hub.as_str())
                && read
                && (scope.key.starts_with("c/") || scope.key.starts_with("h/"))
            {
                assets
                    .library
                    .forget_pulled_of(&scope.key)
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                assets.relook.lock().expect("relook").insert(scope.key);
            }
        }
    }
    assets.metadata_moved.send_modify(|count| *count = count.wrapping_add(1));
    super::session::wake(node, hub);
    Ok(set)
}

/// Each account logged in here, on each hub: whether it takes in its copies,
/// and what of them is held here taken in no more. Call on a thread that may
/// block.
pub(crate) fn report(node: &Node) -> Vec<fastctx_types::data::CopiesOf> {
    let Some(assets) = node.assets.as_ref() else { return Vec::new() };
    let kind = crate::updates::install_kind(node);
    let held = assets.library.held_scopes().unwrap_or_default();
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
            let reader = format!("{account}@{opened}");
            let held_paused: Vec<String> = held
                .iter()
                .filter(|scope| scope.hub.as_deref() == Some(hub.as_str()))
                .filter(|scope| scope.readers.as_deref().unwrap_or_default().lines().any(|line| line == reader))
                .filter(|scope| paused(node, scope, Some(account)))
                .map(|scope| scope.key.clone())
                .collect();
            let bytes = assets
                .library
                .bytes_of_scopes(held_paused.clone())
                .map_or(0, |streams| streams.iter().map(|stream| stream.bytes).sum());
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
                paused_scopes: held_paused.len() as u64,
                paused_bytes: bytes,
            });
        }
    }
    found
}

/// Sets whether `account` of the hub `hub`, logged in here, takes in its
/// copies here, as `source` says (`set`); `opened`, where given, is when the
/// account the asker means was opened: one made again since is not it. Call
/// on a thread that may block.
pub(crate) fn set_here(
    node: &Arc<Node>,
    hub: &str,
    account: &str,
    opened: Option<i64>,
    state: State,
    source: Source,
) -> Result<Set, String> {
    let link = node
        .links()
        .into_iter()
        .find(|link| link.hub().as_str() == hub)
        .ok_or_else(|| format!("this machine is on no network of the hub {hub}"))?;
    let name = fastctx_types::AccountName::try_from(account.to_string()).map_err(|error| error.to_string())?;
    let (hub_id, now) = {
        let view = link.view();
        if !view.me().is_some_and(|me| me.logins.contains(&name)) {
            return Err(format!("{account} is not logged in here on {hub}"));
        }
        let now = view
            .directory
            .account(&name)
            .map(|found| found.opened.as_millisecond())
            .ok_or_else(|| format!("there is no account {account} on {hub}"))?;
        (link.hub().clone(), now)
    };
    if opened.is_some_and(|opened| opened != now) {
        return Err(format!("{account} on {hub} was made again since"));
    }
    set(node, &hub_id, account, now, state, source).map_err(|error| error.to_string())
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
        // Opened as the application: taken in.
        let before = copies.generation();
        assert_eq!(copies.set("h", "a", 1, State::On, Source::App).unwrap(), Set::Changed);
        assert_eq!(copies.state("h", "a", 1, None), State::On);
        assert!(copies.generation() > before);
        assert_eq!(copies.set("h", "a", 1, State::On, Source::App).unwrap(), Set::Unchanged);
        // Paused by its owner, opening the application does not undo it.
        assert_eq!(copies.set("h", "a", 1, State::Paused, Source::Owner).unwrap(), Set::Changed);
        assert_eq!(copies.set("h", "a", 1, State::On, Source::App).unwrap(), Set::OwnersWord);
        assert_eq!(copies.state("h", "a", 1, None), State::Paused);
        // Its owner takes it in again.
        assert_eq!(copies.set("h", "a", 1, State::On, Source::Owner).unwrap(), Set::Changed);
        assert_eq!(Copies::open(&dir).state("h", "a", 1, None), State::On);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_not_read_pauses_nothing() {
        let dir = dir("broken");
        fastctx_platform::disk::write(dir.join(FILE), b"not json").unwrap();
        let copies = Copies::open(&dir);
        assert_eq!(copies.state("h", "a", 1, Some(Kind::Headless)), State::On);
        assert!(copies.set("h", "a", 1, State::Paused, Source::Owner).is_err());
        assert_eq!(fastctx_platform::disk::read(dir.join(FILE)).unwrap(), b"not json");
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }

    #[test]
    fn readers_as_the_hub_says_them() {
        let scope = HeldScope {
            key: "c/h/m/r".into(),
            holding: 1,
            pulled: Some(1),
            allowed: None,
            readers: Some("ada@12\nbob@34\nnot one".into()),
            hub: Some("h".into()),
            description: None,
        };
        assert_eq!(readers(&scope), vec![("ada".to_string(), 12), ("bob".to_string(), 34)]);
    }
}
