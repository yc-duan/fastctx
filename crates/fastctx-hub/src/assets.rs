//! The hub's side of the second version's asset sync (design §6, §17): its
//! service key, its own history as a chain whose head is kept outside its
//! directory, its owner libraries with each second's checkpoint of them,
//! and the seal it keeps over every asset when it finds, on opening, that
//! it was put back from an older copy.

pub(crate) mod answering;
pub(crate) mod apart;
pub(crate) mod bodies;
pub(crate) mod brought;
pub(crate) mod chain;
pub(crate) mod deleted;
#[cfg(test)]
pub(crate) mod drill_kinds;
// They commit records unjudged: the `content-drill` feature (Cargo.toml).
#[cfg(all(test, feature = "content-drill"))]
mod drills;
mod handback;
mod handback_repair;
mod holdings;
mod keys;
pub(crate) mod mirror;
pub(crate) mod told;
pub(crate) mod work;
pub(crate) mod hub_log;
pub(crate) mod ledger_views;
pub(crate) mod line;
pub(crate) mod restores;
pub mod recovery;
pub(crate) mod objects;
mod owners;
pub(crate) mod proving;
mod reclaim;
mod scopes;
mod sentinel_homes;
pub(crate) mod serve;
pub(crate) mod standing;
pub(crate) mod witness;

pub(crate) use owners::Owner;
pub(crate) use witness::Witness;

use fastctx_content::v2::{Head, HubSigner};
use fastctx_storage::Database;
use fastctx_storage::rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// The mark a backup of the hub carries and a live hub's directory never
/// does (design §17, item 1): a hub opened with it here was put back.
pub(crate) const BACKUP_MARKER: &str = "backup-marker";
/// Why the assets are sealed when the hub's setting seals them at each
/// start (design §17 (b)).
const SEALED_AT_START: &str = restores::AT_START;

/// The asset side's tables in the hub's control database.
pub(crate) fn schema() -> String {
    format!(
        "{}{}{}{}{}{}{}",
        chain::SCHEMA,
        scopes::SCHEMA,
        bodies::SCHEMA,
        standing::SCHEMA,
        ledger_views::SCHEMA,
        sentinel_homes::SCHEMA,
        handback::SCHEMA
    )
}

/// Brings the asset side's tables in the control database up to this
/// release, after `schema` made what was missing.
pub(crate) fn migrate(c: &fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()> {
    brought::migrate(c)?;
    bodies::migrate(c)?;
    scopes::migrate(c)
}

/// The hub's incarnation: made with its asset side and stamped on every
/// owner library it makes (design §17, item 3).
fn incarnation(control: &Database) -> io::Result<Vec<u8>> {
    control
        .write_now(128, |c| {
            let kept: Option<Vec<u8>> = c
                .prepare_cached("SELECT value FROM hub_meta WHERE name = 'assets.incarnation'")?
                .query_row([], |row| row.get(0))
                .optional()?;
            if let Some(kept) = kept {
                return Ok(kept);
            }
            let made = fastctx_net::ids::new_hub_id().map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
            let made = made.as_str().as_bytes().to_vec();
            c.prepare_cached("INSERT INTO hub_meta (name, value) VALUES ('assets.incarnation', ?1)")?.execute([&made])?;
            Ok(made)
        })
        .map_err(io::Error::other)
}

/// How long no one may read any scope of an owner before its library goes:
/// long enough that no passing state of the directory takes it.
const UNREAD_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How often owners are looked at for whether anyone reads them.
const SWEEP_EVERY: u32 = 600;

/// The longest pause between two waits of sealing on the control writer.
const BARRIER_PAUSE_MOST: std::time::Duration = std::time::Duration::from_secs(30);

/// A cleanup that saw the hub sealed as it ends goes back whole (AE02).
fn unsealed(sealing: &std::sync::atomic::AtomicBool) -> fastctx_storage::Result<()> {
    if sealing.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(fastctx_storage::Error::new("the hub sealed while it cleaned"));
    }
    Ok(())
}

/// Whether `current` is the very directory `judged` was taken from: every
/// part the same shared one, as a directory no edit touched since is.
fn same_directory(judged: &fastctx_net::directory::Directory, current: &fastctx_net::directory::Directory) -> bool {
    fn same<T>(a: &[Arc<T>], b: &[Arc<T>]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| Arc::ptr_eq(a, b))
    }
    same(&judged.accounts, &current.accounts)
        && same(&judged.members, &current.members)
        && same(&judged.shares, &current.shares)
        && same(&judged.assists, &current.assists)
        && same(&judged.plans, &current.plans)
        && same(&judged.acting, &current.acting)
        && same(&judged.projects, &current.projects)
}

/// How often the owner libraries are checkpointed.
pub(crate) const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);

/// The epochs and gaps of a stream already in the hub's chain, by their
/// keys and digests.
struct Witnessed {
    epochs: BTreeSet<(i64, Vec<u8>)>,
    gaps: BTreeSet<(i64, i64, Vec<u8>)>,
}

/// Scopes checkpointed in one round: a backlog goes in pieces, each within
/// a commit's budget, the rest in the rounds after.
const CHECKPOINT_AT_ONCE: usize = 4096;

/// The file in a hub's directory that, there as the hub opens, has
/// conversations' streams come through the hub again
/// (`Assets::sync_version`).
const THROUGH_HUB: &str = "conversations-through-hub";

pub(crate) struct Assets {
    /// The hub's id, which each descriptor names (design §2.1): a scope of
    /// one hub is no other's.
    pub(crate) id: String,
    /// The hub's directory, where its operator's files are (`reclaim`).
    root: std::path::PathBuf,
    /// Whether conversations' streams come through the hub as before
    /// `DIRECT`: a file named `THROUGH_HUB` stood in the hub's directory as
    /// it opened. What the machines kept since goes up from where the hub
    /// has it, and nothing is lost either way.
    through_hub: bool,
    /// Where each stream a machine keeps stood as that machine last said
    /// (`Request::Stirred`): said again no further on, nobody is told again.
    stirred: Mutex<BTreeMap<(String, String), u64>>,
    keys: Arc<keys::Keys>,
    pub(crate) owners: owners::Owners,
    pub(crate) witness: Arc<Witness>,
    pub(crate) control: Database,
    /// Why every asset is sealed, when the hub found it was put back
    /// (design §17): nothing is read or taken until recovery ends.
    sealed: Mutex<Option<String>>,
    /// Set as the hub seals, for work already handed to the database's
    /// writer to see when it runs (AD02).
    sealing: Arc<std::sync::atomic::AtomicBool>,
    /// Told as the hub seals for a recovery while it runs: every session is
    /// given a view that says so, and its machines take no call from one
    /// another meanwhile (design §28.41). Set once, as the hub starts.
    pub(crate) on_seal: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// The scopes of each owner library changed since the last checkpoint.
    changed: Mutex<BTreeMap<Owner, BTreeSet<String>>>,
    /// Scopes described since the hub's records were last written into
    /// them (design §28.17).
    pub(crate) described: Mutex<Vec<String>>,
    /// The owner the last checkpoint stopped at: the next goes on after it,
    /// so every owner comes round.
    turn: Mutex<Option<Owner>>,
    /// Within each owner, the scope the last round stopped at.
    scope_turns: Mutex<BTreeMap<Owner, String>>,
    /// One checkpoint written at a time: the round's, or one a window asks
    /// for (II08).
    checkpointing: Mutex<()>,
    /// The machines that read each scope lately: told when a stream of it
    /// moves (design §6.6).
    holders: Mutex<BTreeMap<String, BTreeSet<fastctx_types::MachineName>>>,
    /// The machines that asked what changed in each owner library, by the
    /// library's key: told when a stream of it moves (§28.1).
    followers: Mutex<BTreeMap<String, BTreeSet<fastctx_types::MachineName>>>,
    /// The machines given a receipt of each scope since the last
    /// checkpoint: told when a checkpoint covers it (design §17).
    receipted: Mutex<BTreeMap<String, BTreeSet<fastctx_types::MachineName>>>,
    /// What is to be told unasked, by whom answers a request.
    pushes: Mutex<Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)>>,
    /// The tickets for objects' data connections, and the proofs of
    /// holding that stand (design §6.5, §7.3).
    pub(crate) objects: objects::Objects,
    /// The asset lines open, by member (`line`).
    pub(crate) lines: line::Lines,
    /// The turns asset requests take (`work`).
    pub(crate) work: work::Work,
    /// The scopes deleted with their conversations (`deleted`).
    deleted: deleted::Deleted,
    /// What several accounts own together, and who they are (design §6.2).
    pub(crate) bodies: Arc<bodies::Bodies>,
    /// Where machines stand in syncing (design §28.9).
    pub(crate) standing: standing::Standing,
    /// How many times the hub was sealed for a recovery (§29, 2.2).
    restores: std::sync::atomic::AtomicU64,
}

impl Assets {
    /// Opens the asset side of the hub `id` kept in `dir`, whose control
    /// database is `control`, and checks it was not put back (design §17,
    /// items 1 and 2). What the database committed and a crash kept from
    /// the witness reaches it now, before anything is published or
    /// answered; every scope is looked at again by the checkpoints, so what
    /// changed before a stop is not left out. Call on a thread that may
    /// block.
    pub(crate) fn open(dir: &Path, id: &str, control: Database) -> io::Result<Self> {
        let keys = Arc::new(keys::Keys::open(&dir.join("keys"))?);
        let witness = Arc::new(Witness::open(dir, id)?);
        let incarnation = incarnation(&control)?;
        // The count the directory kept or the witness did, whichever is
        // more, the witness brought up to it (§29, 2.2).
        let restored = restores::count(&control)?.max(witness.recoveries());
        witness.recovered(restored)?;
        let owners = owners::Owners::new(dir.join("assets"), incarnation);
        let assets = Self {
            id: id.to_string(),
            root: dir.to_path_buf(),
            through_hub: fastctx_platform::disk::is_there(dir.join(THROUGH_HUB)),
            stirred: Mutex::new(BTreeMap::new()),
            keys,
            owners,
            witness,
            control: control.clone(),
            sealed: Mutex::new(None),
            sealing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            on_seal: std::sync::OnceLock::new(),
            changed: Mutex::new(BTreeMap::new()),
            described: Mutex::new(Vec::new()),
            turn: Mutex::new(None),
            scope_turns: Mutex::new(BTreeMap::new()),
            checkpointing: Mutex::new(()),
            holders: Mutex::new(BTreeMap::new()),
            followers: Mutex::new(BTreeMap::new()),
            receipted: Mutex::new(BTreeMap::new()),
            pushes: Mutex::new(Vec::new()),
            objects: objects::Objects::default(),
            lines: line::Lines::default(),
            work: work::Work::default(),
            deleted: deleted::Deleted::default(),
            bodies: Arc::new(bodies::Bodies::new(control.clone())),
            standing: standing::Standing::default(),
            restores: std::sync::atomic::AtomicU64::new(restored),
        };
        if let Some(why) = assets.put_back(dir)? {
            // Kept too, so the count of recoveries goes up once for this one
            // (§29, 2.2).
            assets.keep_seal(&why)?;
            assets.witness.freeze();
            *assets.sealed.lock().expect("seal") = Some(why);
            return Ok(assets);
        }
        // Found put back before a stop, it stays sealed for its recovery
        // until it is reopened (AK01).
        let kept: Option<String> = assets
            .control
            .read_now(|c| Ok(c.prepare_cached("SELECT why FROM asset_seal WHERE id = 1")?.query_row([], |row| row.get(0)).optional()?))
            .map_err(io::Error::other)?;
        if let Some(why) = kept {
            assets.witness.freeze();
            *assets.sealed.lock().expect("seal") = Some(why);
            return Ok(assets);
        }
        // What was brought and kept, the seal it showed not yet written as
        // the hub stopped, seals it now as it would have (AL01).
        if let Some(why) = brought::put_back(&assets)? {
            assets.keep_seal(&why)?;
            assets.witness.freeze();
            *assets.sealed.lock().expect("seal") = Some(why);
            return Ok(assets);
        }
        assets.catch_up()?;
        let all: Vec<(String, String)> = assets
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT owner, key FROM asset_scopes")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let mut changed = assets.changed.lock().expect("changed scopes");
        for (owner, scope) in all {
            if let Some(owner) = Owner::from_key(&owner) {
                changed.entry(owner).or_default().insert(scope);
            }
        }
        drop(changed);
        Ok(assets)
    }

    /// Deletes the libraries none of whose scopes anyone has read for
    /// `UNREAD_FOR` (design §6.2). An owner read again before then is kept;
    /// one whose readers cannot be known is judged again later, never taken
    /// for no one's. Its deletion begins in one commit, in the chain and the
    /// witness, only while it holds exactly the scopes judged; from then no
    /// scope is described into it nor its library opened, and its records
    /// then its files go a batch at a time (`finish_deleting`). How many
    /// began. Call on a thread that may block.
    pub(crate) fn sweep_owners(&self, directory: &fastctx_net::directory_index::Indexed<'_>) -> io::Result<usize> {
        if self.sealed().is_some() || directory.all.accounts.is_empty() {
            return Ok(0);
        }
        self.finish_deleting()?;
        let now = jiff::Timestamp::now().as_millisecond();
        let (mut dropped, mut after) = (0, String::new());
        loop {
            let from = after.clone();
            let owners: Vec<String> = self
                .control
                .read_now(move |c| {
                    let mut statement = c.prepare_cached(
                        "SELECT DISTINCT owner FROM asset_scopes WHERE owner > ?1
                         AND owner NOT IN (SELECT owner FROM asset_deleting) ORDER BY owner LIMIT 256",
                    )?;
                    let rows = statement.query_map([&from], |row| row.get(0))?;
                    Ok(rows.collect::<std::result::Result<_, _>>()?)
                })
                .map_err(io::Error::other)?;
            let Some(last) = owners.last() else { break };
            after = last.clone();
            for owner in owners {
                if self.sweep_owner(directory, &owner, now)? {
                    dropped += 1;
                }
            }
        }
        // A library all of whose scopes went is the owner of none: found on
        // the disk, it is judged as any other — none reads what it does not
        // hold — and goes once unread as long.
        let named: std::collections::HashSet<String> = self
            .control
            .read_now(|c| {
                let mut statement =
                    c.prepare_cached("SELECT DISTINCT owner FROM asset_scopes UNION SELECT owner FROM asset_deleting")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        for owner in self.owners.on_disk()? {
            let key = owner.key();
            if !named.contains(&key) && self.sweep_owner(directory, &key, now)? {
                dropped += 1;
            }
        }
        self.finish_deleting()?;
        Ok(dropped)
    }

    /// Judges `owner` and, unread long enough, begins its deletion; whether
    /// it began. Its scopes are judged a page at a time; the deletion begins
    /// under the directory's writer, judged again by the directory as it is
    /// then, and only while the owner holds the very scopes judged.
    fn sweep_owner(&self, directory: &fastctx_net::directory_index::Indexed<'_>, owner: &str, now: i64) -> io::Result<bool> {
        let read = |directory: &fastctx_net::directory_index::Indexed<'_>| -> io::Result<Option<(bool, [u8; 32])>> {
            let (mut digest, mut after) = (Sha256::new(), String::new());
            loop {
                let (key, from) = (owner.to_string(), after.clone());
                let page: Vec<String> = self
                    .control
                    .read_now(move |c| {
                        let mut statement =
                            c.prepare_cached("SELECT key FROM asset_scopes WHERE owner = ?1 AND key > ?2 ORDER BY key LIMIT 4096")?;
                        let rows = statement.query_map(fastctx_storage::rusqlite::params![key, from], |row| row.get(0))?;
                        Ok(rows.collect::<std::result::Result<_, _>>()?)
                    })
                    .map_err(io::Error::other)?;
                let Some(last) = page.last() else { break };
                after = last.clone();
                for key in &page {
                    digest.update((key.len() as u64).to_le_bytes());
                    digest.update(key.as_bytes());
                    let judged = match scopes::scope(self, key) {
                        Ok(Some(scope)) => scopes::anyone_reads(directory, &scope),
                        Ok(None) => Ok(false),
                        Err(why) => Err(io::Error::other(why)),
                    };
                    match judged {
                        // One reader keeps the owner: nothing more to judge.
                        Ok(true) => return Ok(Some((true, [0; 32]))),
                        Ok(false) => {}
                        // Not known now: judged again at a later sweep.
                        Err(_) => return Ok(None),
                    }
                }
            }
            Ok(Some((false, digest.finalize().into())))
        };
        let Some((anyone, first)) = read(directory)? else { return Ok(false) };
        let key = owner.to_string();
        if anyone {
            self.control
                .write_now(128, move |c| {
                    c.prepare_cached("DELETE FROM asset_unread WHERE owner = ?1")?.execute([&key])?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
            return Ok(false);
        }
        let since: i64 = self
            .control
            .write_now(128, move |c| {
                c.prepare_cached("INSERT OR IGNORE INTO asset_unread (owner, since) VALUES (?1, ?2)")?
                    .execute(fastctx_storage::rusqlite::params![key, now])?;
                Ok(c.prepare_cached("SELECT since FROM asset_unread WHERE owner = ?1")?.query_row([&key], |row| row.get(0))?)
            })
            .map_err(io::Error::other)?;
        if now - since < UNREAD_FOR.as_millis() as i64 {
            return Ok(false);
        }
        let Some(found) = Owner::from_key(owner) else { return Ok(false) };
        let digest: [u8; 32] = Sha256::digest(owner.as_bytes()).into();
        let body = match &found {
            Owner::Body(id) => Some(id.clone()),
            _ => None,
        };
        // The seal before the directory's writer, as every grant takes them:
        // a hub found put back deletes nothing more (AC03).
        let seal = self.seal_held();
        if seal.is_some() {
            return Ok(false);
        }
        let begun = self.bodies.under_directory(directory.all, |current| -> io::Result<Option<(u64, [u8; 32])>> {
            // The directory as it is now: a project given back, an account
            // restored, may read it again. Unchanged since the judgment, the
            // judgment stands (AC02).
            let judged = if same_directory(directory.all, current) {
                first
            } else {
                let index = fastctx_net::directory_index::Index::new(current);
                let Some((anyone, judged)) = read(&fastctx_net::directory_index::Indexed::new(current, &index))? else { return Ok(None) };
                if anyone {
                    return Ok(None);
                }
                judged
            };
            let (key, sealing) = (owner.to_string(), self.sealing.clone());
            self.control
                .write_now(1024, move |c| {
                    // Sealed since it was handed over, nothing is begun (AD02).
                    if sealing.load(std::sync::atomic::Ordering::SeqCst) {
                        return Ok(None);
                    }
                    // A scope described into it since it was judged keeps it.
                    let mut statement = c.prepare_cached("SELECT key FROM asset_scopes WHERE owner = ?1 ORDER BY key")?;
                    let mut rows = statement.query([&key])?;
                    let mut still = Sha256::new();
                    while let Some(row) = rows.next()? {
                        let scope: String = row.get(0)?;
                        still.update((scope.len() as u64).to_le_bytes());
                        still.update(scope.as_bytes());
                    }
                    drop(rows);
                    if <[u8; 32]>::from(still.finalize()) != judged {
                        return Ok(None);
                    }
                    c.prepare_cached("INSERT OR IGNORE INTO asset_deleting (owner) VALUES (?1)")?.execute([&key])?;
                    c.prepare_cached("DELETE FROM asset_unread WHERE owner = ?1")?.execute([&key])?;
                    if let Some(body) = &body {
                        c.prepare_cached("DELETE FROM asset_bodies WHERE id = ?1")?.execute([body])?;
                    }
                    let appended = chain::append(c, "owner-deleted", &digest)?;
                    unsealed(&sealing)?;
                    Ok(Some(appended))
                })
                .map_err(io::Error::other)
        });
        if self.sealing.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }
        let Some((seq, head)) = begun? else { return Ok(false) };
        self.witness.advanced(seq, &head)?;
        drop(seal);
        Ok(true)
    }

    /// Whether the library of `owner` is being deleted.
    fn deleting(&self, owner: &Owner) -> io::Result<bool> {
        let key = owner.key();
        self.control
            .read_now(move |c| Ok(c.prepare_cached("SELECT 1 FROM asset_deleting WHERE owner = ?1")?.query_row([&key], |_| Ok(())).optional()?))
            .map(|found| found.is_some())
            .map_err(io::Error::other)
    }

    /// Removes what remains of the owners being deleted: their records a
    /// batch at a time, then their files once nobody holds their library; one
    /// still open goes at a later sweep.
    fn finish_deleting(&self) -> io::Result<()> {
        // A fence whose deletion is no longer recorded is of one that ended,
        // though whoever ended it never heard so (AC01).
        for owner in self.owners.fenced() {
            if !self.deleting(&owner)? {
                self.owners.deleted(&owner);
            }
        }
        let deleting: Vec<String> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT owner FROM asset_deleting")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        for key in deleting {
            let Some(owner) = Owner::from_key(&key) else { continue };
            loop {
                // Nothing goes once the hub is found put back (AC03): the seal
                // is held while each batch goes.
                let seal = self.seal_held();
                if seal.is_some() {
                    return Ok(());
                }
                let (batch, sealing) = (key.clone(), self.sealing.clone());
                let removed = self
                    .control
                    .write_now(1024, move |c| {
                        // Sealed since it was handed over, nothing goes (AD02).
                        if sealing.load(std::sync::atomic::Ordering::SeqCst) {
                            return Ok(0);
                        }
                        let mut removed = c
                            .prepare_cached("DELETE FROM asset_scopes WHERE key IN (SELECT key FROM asset_scopes WHERE owner = ?1 LIMIT 4096)")?
                            .execute([&batch])?;
                        // A batch of rows at a time, by each table's own key.
                        for (table, key) in [
                            ("hub_checkpoints", "owner, scope"),
                            ("hub_epoch_facts", "owner, scope, stream, epoch"),
                            ("hub_gap_facts", "owner, scope, stream, epoch, from_sequence"),
                        ] {
                            removed += c
                                .prepare_cached(&format!(
                                    "DELETE FROM {table} WHERE ({key}) IN (SELECT {key} FROM {table} WHERE owner = ?1 LIMIT 4096)"
                                ))?
                                .execute([&batch])?;
                        }
                        unsealed(&sealing)?;
                        Ok(removed)
                    })
                    .map_err(io::Error::other);
                if self.sealing.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
                let removed = removed?;
                drop(seal);
                if removed == 0 {
                    break;
                }
            }
            let seal = self.seal_held();
            if seal.is_some() {
                return Ok(());
            }
            if self.owners.delete(&owner).is_ok() {
                let sealing = self.sealing.clone();
                let done = self
                    .control
                    .write_now(128, move |c| {
                        unsealed(&sealing)?;
                        c.prepare_cached("DELETE FROM asset_deleting WHERE owner = ?1")?.execute([&key])?;
                        unsealed(&sealing)
                    })
                    .map_err(io::Error::other);
                if self.sealing.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
                done?;
                self.owners.deleted(&owner);
            }
            drop(seal);
        }
        // What was moved aside goes outside the seal: the deletions were
        // decided under it.
        self.owners.discard()
    }

    /// The chain's latest head reaches the witness, if it had not.
    pub(crate) fn catch_up(&self) -> io::Result<()> {
        if let Some((seq, head)) = self.control.read_now(chain::latest).map_err(io::Error::other)? {
            self.witness.advanced(seq, &head)?;
        }
        Ok(())
    }

    /// Why the hub was put back, if it was: its backup's mark is here, or
    /// the witness outside its directory holds a chain head its database
    /// does not. Each owner library is checked as it opens (`library`).
    fn put_back(&self, dir: &Path) -> io::Result<Option<String>> {
        if fastctx_platform::disk::exists(dir.join(BACKUP_MARKER))? {
            return Ok(Some("the hub was opened from a backup".into()));
        }
        let kept = self.witness.kept();
        if kept.seq == 0 {
            return Ok(None);
        }
        let seq = kept.seq;
        let head = self.control.read_now(move |c| chain::head_at(c, seq)).map_err(io::Error::other)?;
        if head.map(|head| witness::hex(&head)) != Some(kept.head) {
            return Ok(Some(format!(
                "the hub's directory is older than what it last wrote outside it ({})",
                self.witness.path().display()
            )));
        }
        Ok(None)
    }

    /// The library of `owner`, checked as it opens against the hub's own
    /// record of it (design §17, item 3): a library older than its latest
    /// checkpoint or than an epoch the hub opened in it, or another hub's,
    /// seals every asset.
    pub(crate) fn library(&self, owner: &Owner) -> io::Result<fastctx_content::v2::Library> {
        // A library being deleted is opened by no one (design §6.2).
        if self.deleting(owner)? {
            return Err(io::Error::other(format!("the library of {} is being deleted", owner.key())));
        }
        match self.owners.library(owner, |library| self.behind(owner, library))? {
            Ok(library) => Ok(library),
            Err(why) => {
                self.seal(&why);
                Err(io::Error::other(why))
            }
        }
    }

    /// The latest head each of `scopes` of `owner` was checkpointed at.
    fn checkpointed_of(&self, owner: &Owner, scopes: &[String]) -> io::Result<BTreeMap<String, Head>> {
        let (key, scopes) = (owner.key(), scopes.to_vec());
        self.control
            .read_now(move |c| {
                let mut statement =
                    c.prepare_cached("SELECT change, parked, prefix, accumulated FROM hub_checkpoints WHERE owner = ?1 AND scope = ?2")?;
                let mut heads = BTreeMap::new();
                for scope in scopes {
                    let found = statement
                        .query_row(fastctx_storage::rusqlite::params![key, scope], |row| {
                            let digest = |bytes: Vec<u8>| <[u8; 32]>::try_from(bytes).unwrap_or([0; 32]);
                            Ok(Head {
                                scope: scope.clone(),
                                change: row.get::<_, i64>(0)? as u64,
                                parked: row.get::<_, i64>(1)? as u64,
                                prefix: digest(row.get(2)?),
                                accumulated: digest(row.get(3)?),
                            })
                        })
                        .optional()?;
                    if let Some(head) = found {
                        heads.insert(scope, head);
                    }
                }
                Ok(heads)
            })
            .map_err(io::Error::other)
    }

    /// The latest head each scope of `owner` was checkpointed at.
    pub(crate) fn checkpointed(&self, owner: &Owner) -> io::Result<BTreeMap<String, Head>> {
        let key = owner.key();
        self.control
            .read_now(move |c| {
                let mut statement =
                    c.prepare_cached("SELECT scope, change, parked, prefix, accumulated FROM hub_checkpoints WHERE owner = ?1")?;
                let rows = statement.query_map([&key], |row| {
                    let digest = |bytes: Vec<u8>| <[u8; 32]>::try_from(bytes).unwrap_or([0; 32]);
                    Ok(Head {
                        scope: row.get(0)?,
                        change: row.get::<_, i64>(1)? as u64,
                        parked: row.get::<_, i64>(2)? as u64,
                        prefix: digest(row.get(3)?),
                        accumulated: digest(row.get(4)?),
                    })
                })?;
                let mut heads = BTreeMap::new();
                for head in rows {
                    let head = head?;
                    heads.insert(head.scope.clone(), head);
                }
                Ok(heads)
            })
            .map_err(io::Error::other)
    }

    /// Why `owner`'s library may not be served, if it may not: it is older
    /// than a checkpoint the hub's database holds of it, or lacks an epoch
    /// the hub opened in it.
    fn behind(&self, owner: &Owner, library: &fastctx_content::v2::Library) -> io::Result<Option<String>> {
        // What a scope deleted before the hub stopped left in it goes on
        // going (`deleted`).
        self.library_opened(owner, library);
        let key = owner.key();
        for head in self.checkpointed(owner)?.values() {
            if !library.holds(head).map_err(|error| io::Error::other(error.to_string()))? {
                return Ok(Some(format!("the library of {key} is older than the hub's own record of it")));
            }
        }
        let owner_key = key.clone();
        let epochs: Vec<(String, String, i64, Vec<u8>)> = self
            .control
            .read_now(move |c| {
                let mut statement = c.prepare_cached(
                    "SELECT scope, stream, epoch, digest FROM hub_epoch_facts f WHERE owner = ?1 AND epoch =
                     (SELECT MAX(epoch) FROM hub_epoch_facts g WHERE g.owner = f.owner AND g.scope = f.scope AND g.stream = f.stream)",
                )?;
                let rows = statement.query_map([&owner_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        for (scope, stream, epoch, digest) in epochs {
            let digest = <[u8; 32]>::try_from(digest).unwrap_or([0; 32]);
            if !library.holds_epoch(&scope, &stream, epoch as u64, &digest).map_err(|error| io::Error::other(error.to_string()))? {
                return Ok(Some(format!("the library of {key} lacks an epoch the hub opened in {scope}")));
            }
        }
        let owner_key = key.clone();
        let gaps: Vec<(String, String, i64, i64, Vec<u8>)> = self
            .control
            .read_now(move |c| {
                let mut statement =
                    c.prepare_cached("SELECT scope, stream, epoch, from_sequence, digest FROM hub_gap_facts WHERE owner = ?1")?;
                let rows = statement.query_map([&owner_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        for (scope, stream, epoch, from, digest) in gaps {
            let digest = <[u8; 32]>::try_from(digest).unwrap_or([0; 32]);
            if !library.holds_gap(&scope, &stream, epoch as u64, from as u64, &digest).map_err(|error| io::Error::other(error.to_string()))? {
                return Ok(Some(format!("the library of {key} lacks a gap the hub signed in {scope}")));
            }
        }
        Ok(None)
    }

    pub(crate) fn keys(&self) -> &keys::Keys {
        &self.keys
    }

    /// Whether a verified chain head belongs to history retired at recovery.
    pub(crate) fn closed_head(&self, seq: u64, head: &[u8; 32]) -> io::Result<bool> {
        let head = *head;
        self.control.read_now(move |c| chain::closed_head(c, seq, &head)).map_err(io::Error::other)
    }

    /// Whether a receipt naming `head` is of a history the hub set aside
    /// when it recovered: exactly one it was shown then.
    pub(crate) fn closed_receipt(&self, head: &Head) -> io::Result<bool> {
        let head = head.clone();
        self.control
            .read_now(move |c| {
                Ok(c.prepare_cached(
                    "SELECT 1 FROM closed_receipts WHERE scope = ?1 AND change = ?2 AND prefix = ?3 AND parked = ?4 AND accumulated = ?5",
                )?
                .query_row(
                    fastctx_storage::rusqlite::params![head.scope, head.change as i64, &head.prefix[..], head.parked as i64, &head.accumulated[..]],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
            })
            .map_err(io::Error::other)
    }

    /// What `machine` brought while every asset was sealed, as far as the
    /// hub's word stands behind it: the head it saw and the receipts it
    /// held, kept for the recovery (design §17).
    /// When it was kept: what `judged` marks as judged.
    pub(crate) fn brought(&self, machine: &fastctx_types::MachineName, seen: Option<(u64, [u8; 32])>, receipts: Vec<Head>) -> i64 {
        let (name, at) = (machine.to_string(), jiff::Timestamp::now().as_millisecond());
        let _ = self.control.write_now(256 + receipts.len() * 128, move |c| {
            if let Some((seq, head)) = seen {
                c.prepare_cached("INSERT OR IGNORE INTO recovery_heads (seq, head, machine, at) VALUES (?1, ?2, ?3, ?4)")?
                    .execute(fastctx_storage::rusqlite::params![seq as i64, &head[..], name, at])?;
            }
            for head in &receipts {
                c.prepare_cached(
                    "INSERT OR IGNORE INTO recovery_receipts (scope, change, prefix, parked, accumulated, machine, at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?
                .execute(fastctx_storage::rusqlite::params![
                    head.scope,
                    head.change as i64,
                    &head.prefix[..],
                    head.parked as i64,
                    &head.accumulated[..],
                    name,
                    at
                ])?;
            }
            Ok(())
        });
        at
    }

    /// `machine` opened an asset session: it speaks this asset sync, and so
    /// clears for a share that waits for it (design §6.3).
    pub(crate) fn speaks(&self, key: &fastctx_net::sign::PublicKey) -> io::Result<()> {
        let name = witness::hex(key.as_bytes());
        let known = self
            .control
            .read_now({
                let name = name.clone();
                move |c| Ok(c.prepare_cached("SELECT 1 FROM asset_machines WHERE machine = ?1")?.query_row([&name], |_| Ok(())).optional()?)
            })
            .map_err(io::Error::other)?;
        if known.is_none() {
            self.control
                .write_now(128, move |c| {
                    c.prepare_cached("INSERT OR IGNORE INTO asset_machines (machine) VALUES (?1)")?.execute([&name])?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
        }
        Ok(())
    }

    /// Whether the machine whose key is `key` ever opened an asset session
    /// here: by its key, whatever it is called.
    pub(crate) fn spoken(&self, key: &fastctx_net::sign::PublicKey) -> bool {
        let name = witness::hex(key.as_bytes());
        self.control
            .read_now(move |c| Ok(c.prepare_cached("SELECT 1 FROM asset_machines WHERE machine = ?1")?.query_row([&name], |_| Ok(())).optional()?))
            .is_ok_and(|found| found.is_some())
    }

    /// The epoch descriptors and gap facts of `stream` of `scope` in
    /// `owner`'s library, about to be handed out: each not in the hub's
    /// chain yet goes into it and the witness first (design §6.1, §17), so a
    /// library put back to before one is found as it opens.
    pub(crate) fn witness_facts(
        &self,
        owner: &Owner,
        scope: &str,
        stream: &str,
        epochs: &[fastctx_content::v2::Signed],
        gaps: &[fastctx_content::v2::Signed],
    ) -> io::Result<()> {
        let (owner_key, scope_key, stream_name) = (owner.key(), scope.to_string(), stream.to_string());
        let known: Witnessed = self
            .control
            .read_now(move |c| {
                let mut epochs = BTreeSet::new();
                let mut statement =
                    c.prepare_cached("SELECT epoch, digest FROM hub_epoch_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3")?;
                for row in statement.query_map(fastctx_storage::rusqlite::params![owner_key, scope_key, stream_name], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })? {
                    epochs.insert(row?);
                }
                let mut gaps = BTreeSet::new();
                let mut statement = c.prepare_cached(
                    "SELECT epoch, from_sequence, digest FROM hub_gap_facts WHERE owner = ?1 AND scope = ?2 AND stream = ?3",
                )?;
                for row in statement.query_map(fastctx_storage::rusqlite::params![owner_key, scope_key, stream_name], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })? {
                    gaps.insert(row?);
                }
                Ok(Witnessed { epochs, gaps })
            })
            .map_err(io::Error::other)?;
        let mut new_epochs = Vec::new();
        for signed in epochs {
            let fact = fastctx_content::v2::EpochFact::decode(&signed.fact).map_err(io::Error::other)?;
            let digest: [u8; 32] = Sha256::digest(&signed.fact).into();
            if !known.epochs.contains(&(fact.epoch as i64, digest.to_vec())) {
                new_epochs.push((fact.epoch as i64, digest));
            }
        }
        let mut new_gaps = Vec::new();
        for signed in gaps {
            let fact = fastctx_content::v2::GapFact::decode(&signed.fact).map_err(io::Error::other)?;
            let digest: [u8; 32] = Sha256::digest(&signed.fact).into();
            if !known.gaps.contains(&(fact.epoch as i64, fact.from as i64, digest.to_vec())) {
                new_gaps.push((fact.epoch as i64, fact.from as i64, digest));
            }
        }
        if new_epochs.is_empty() && new_gaps.is_empty() {
            return self.catch_up();
        }
        let mut hasher = Sha256::new();
        hasher.update(b"fastctx-hub-facts/1");
        for (_, digest) in &new_epochs {
            hasher.update(digest);
        }
        for (_, _, digest) in &new_gaps {
            hasher.update(digest);
        }
        let all: [u8; 32] = hasher.finalize().into();
        let (owner_key, scope_key, stream_name) = (owner.key(), scope.to_string(), stream.to_string());
        let (seq, head) = self
            .control
            .write_now(512 + 96 * (new_epochs.len() + new_gaps.len()), move |c| {
                let (seq, head) = chain::append(c, "facts", &all)?;
                for (epoch, digest) in &new_epochs {
                    c.prepare_cached(
                        "INSERT OR REPLACE INTO hub_epoch_facts (owner, scope, stream, epoch, digest, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?
                    .execute(fastctx_storage::rusqlite::params![owner_key, scope_key, stream_name, epoch, &digest[..], seq as i64])?;
                }
                for (epoch, from, digest) in &new_gaps {
                    c.prepare_cached(
                        "INSERT OR REPLACE INTO hub_gap_facts (owner, scope, stream, epoch, from_sequence, digest, seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )?
                    .execute(fastctx_storage::rusqlite::params![owner_key, scope_key, stream_name, epoch, from, &digest[..], seq as i64])?;
                }
                Ok((seq, head))
            })
            .map_err(io::Error::other)?;
        self.witness.advanced(seq, &head)
    }

    /// An epoch the hub just opened in `scope` of `owner`'s library goes into
    /// its chain and the witness before the device hears of it (design
    /// §6.1): a library put back to before it is found as it opens.
    pub(crate) fn opened_epoch(&self, owner: &Owner, scope: &str, stream: &str, epoch: u64, fact: &[u8]) -> io::Result<()> {
        let digest: [u8; 32] = Sha256::digest(fact).into();
        let (owner, scope, stream) = (owner.key(), scope.to_string(), stream.to_string());
        let (seq, head) = self
            .control
            .write_now(512, move |c| {
                let (seq, head) = chain::append(c, "epoch", &digest)?;
                c.prepare_cached(
                    "INSERT OR REPLACE INTO hub_epoch_facts (owner, scope, stream, epoch, digest, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?
                .execute(fastctx_storage::rusqlite::params![owner, scope, stream, epoch as i64, &digest[..], seq as i64])?;
                Ok((seq, head))
            })
            .map_err(io::Error::other)?;
        self.witness.advanced(seq, &head)
    }

    /// Seals every asset from now on (design §17): the hub found it was put
    /// back. The witness is left as the evidence of the lost history.
    pub(crate) fn seal(&self, why: &str) {
        // The witness is frozen under the seal: a grant that holds it while
        // it commits is witnessed before anything seals.
        let mut sealed = self.sealed.lock().expect("seal");
        self.sealing.store(true, std::sync::atomic::Ordering::SeqCst);
        // Sealed for its recovery from now on, kept so across a restart
        // (AK01): a seal for a recovery already there stays as it is.
        let replaces = sealed.as_deref().is_none_or(|was| was == SEALED_AT_START);
        let (kept, at) = (why.to_string(), jiff::Timestamp::now().as_millisecond());
        // What the control writer began before it saw the seal ends before
        // the witness freezes (AE02): a write queued behind it is answered
        // only once it committed or went back. One that did not answer says
        // nothing of what is under way, so it is asked again until one does
        // (AF01); every cleanup also looks at the seal again as it ends.
        let mut pause = std::time::Duration::from_secs(1);
        // The count of recoveries goes up as this one's seal is first kept
        // (§29, 2.2).
        let witnessed = self.witness.recoveries();
        let keep = move |c: &fastctx_storage::rusqlite::Connection| {
            if replaces {
                return Ok(Some(restores::seal_in(c, &kept, at, witnessed)?));
            }
            Ok(None)
        };
        loop {
            match self.control.write_now(512, keep.clone()) {
                Ok(count) => {
                    if let Some(count) = count {
                        self.restores.store(count, std::sync::atomic::Ordering::SeqCst);
                        // Kept past the directory: a failure here leaves the
                        // witness behind, brought up again as the hub opens
                        // or reopens, while it is sealed and numbers nothing.
                        if let Err(error) = self.witness.recovered(count) {
                            eprintln!("keeping the count of recoveries in the hub's witness: {error}");
                        }
                    }
                    break;
                }
                Err(error) => {
                    eprintln!("sealing the hub's assets waits for its control database: {error}");
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(BARRIER_PAUSE_MOST);
                }
            }
        }
        self.witness.freeze();
        // Found put back, the hub is sealed for its recovery: a seal it put
        // on only as it started gives way, so the super administrator no
        // longer opens it, `fastctx hub reopen` does.
        if replaces {
            *sealed = Some(why.to_string());
        }
        // Told with the seal let go: the views it makes read it.
        drop(sealed);
        if replaces && let Some(told) = self.on_seal.get() {
            told();
        }
    }

    /// Whether `why`, a seal as it was read, is the one the hub puts on as
    /// it starts.
    pub(crate) fn is_start_seal(why: &str) -> bool {
        why == SEALED_AT_START
    }

    /// Whether `stream` of `scope`, which a machine keeps, is said to stand
    /// further on than it was last said to: kept as where it stands now.
    pub(crate) fn stirred(&self, scope: &str, stream: &str, next: u64) -> bool {
        let mut stirred = self.stirred.lock().expect("stirred");
        let known = stirred.entry((scope.to_string(), stream.to_string())).or_insert(0);
        let further = next > *known;
        if further {
            *known = next;
        }
        further
    }

    /// The asset sync version this hub tells machines it speaks.
    pub(crate) fn sync_version(&self) -> u32 {
        if self.through_hub { fastctx_types::assets::DIRECT - 1 } else { fastctx_types::assets::VERSION }
    }

    /// The hub's answer that its assets are sealed for `why`: sealed for a
    /// recovery, with the count of recoveries it reopens with, under which
    /// a device hands back what the hub signed past where it was put back
    /// (design §28.38).
    pub(crate) fn sealed_reply(&self, why: String) -> fastctx_types::assets::Reply {
        let recovery = (!Self::is_start_seal(&why)).then(|| self.restores() + 1);
        fastctx_types::assets::Reply::Sealed { why, recovery }
    }

    /// The seal, held: nothing seals the hub while a grant that checked it
    /// commits (design §17).
    pub(crate) fn seal_held(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.sealed.lock().expect("seal")
    }

    /// Why every asset is sealed, if it is.
    pub(crate) fn sealed(&self) -> Option<String> {
        self.sealed.lock().expect("seal").clone()
    }

    /// The least number the hub's sources hand out now (§29, 2.2).
    pub(crate) fn number_floor(&self) -> u64 {
        restores::floor(self.restores())
    }

    /// How many times the hub was sealed for a recovery.
    pub(crate) fn restores(&self) -> u64 {
        self.restores.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Keeps the seal `why` for a recovery, the count of recoveries going up
    /// as it is first kept.
    fn keep_seal(&self, why: &str) -> io::Result<()> {
        let (kept, at, witnessed) = (why.to_string(), jiff::Timestamp::now().as_millisecond(), self.witness.recoveries());
        let count = self.control.write_now(512, move |c| restores::seal_in(c, &kept, at, witnessed)).map_err(io::Error::other)?;
        self.restores.store(count, std::sync::atomic::Ordering::SeqCst);
        self.witness.recovered(count)
    }

    /// Seals every asset as the hub starts, its setting says so (design §17
    /// (b)): nothing was found wrong, so the witness goes on as ever; only
    /// no asset is given or taken until the super administrator opens them
    /// (`Change::OpenAssets`, with `openable`). A seal already there stays.
    pub(crate) fn seal_at_start(&self) {
        let mut sealed = self.sealed.lock().expect("seal");
        if sealed.is_none() {
            *sealed = Some(SEALED_AT_START.to_string());
        }
    }

    /// Whether `sealed`, the seal as held, is one the super administrator
    /// may open: the one the hub put on as it started. A seal for any other
    /// reason stays until `fastctx hub reopen`.
    pub(crate) fn openable(sealed: &Option<String>) -> Result<(), String> {
        match sealed.as_deref() {
            None => Err("the hub's assets are not sealed".into()),
            Some(SEALED_AT_START) => Ok(()),
            Some(why) => Err(format!("the hub's assets are sealed because {why}: stop the hub and run `fastctx hub reopen`")),
        }
    }

    /// The hub's signer of what it says of assets.
    pub(crate) fn signer(&self) -> Arc<dyn HubSigner> {
        Arc::new(keys::Signer { keys: self.keys.clone(), witness: self.witness.clone() })
    }

    pub(crate) fn public(&self) -> fastctx_net::sign::PublicKey {
        self.keys.public()
    }

    /// `scope` of `owner`'s library changed: the next checkpoint takes it.
    pub(crate) fn changed(&self, owner: Owner, scope: &str) {
        self.changed.lock().expect("changed scopes").entry(owner).or_default().insert(scope.to_string());
    }

    /// `machine` reads `scope`: it is told when a stream of it moves.
    pub(crate) fn holds(&self, scope: &str, machine: &fastctx_types::MachineName) {
        self.holders.lock().expect("holders").entry(scope.to_string()).or_default().insert(machine.clone());
    }

    /// `machine` was given a receipt of `scope`: it is told when a
    /// checkpoint covers it.
    pub(crate) fn receipted(&self, scope: &str, machine: &fastctx_types::MachineName) {
        self.receipted.lock().expect("receipted").entry(scope.to_string()).or_default().insert(machine.clone());
    }

    /// `machine` asked what changed in the library `owner`: it is told when
    /// a stream of it moves.
    pub(crate) fn follows(&self, owner: &Owner, machine: &fastctx_types::MachineName) {
        self.followers.lock().expect("followers").entry(owner.key()).or_default().insert(machine.clone());
    }

    /// Whether `owner` has a library on this hub's disk.
    pub(crate) fn has_library(&self, owner: &Owner) -> bool {
        self.owners.exists(owner)
    }

    /// `stream` of `scope`, in `owner`'s library, moved to `next`: each
    /// machine that reads the scope or follows the library, but `by`, is to
    /// be told.
    pub(crate) fn moved(&self, owner: &Owner, scope: &str, stream: &str, next: u64, by: &fastctx_types::MachineName) {
        self.moved_except(owner, scope, stream, next, Some(by));
    }

    /// A stream the hub orders itself moved on (design §28.17): every
    /// machine that holds the scope or follows its library is told.
    pub(crate) fn moved_by_hub(&self, owner: &Owner, scope: &str, stream: &str, next: u64) {
        self.moved_except(owner, scope, stream, next, None);
    }

    fn moved_except(&self, owner: &Owner, scope: &str, stream: &str, next: u64, by: Option<&fastctx_types::MachineName>) {
        self.standing.moved(&owner.key());
        let mut told: BTreeSet<_> = self.holders.lock().expect("holders").get(scope).into_iter().flatten().cloned().collect();
        told.extend(self.followers.lock().expect("followers").get(&owner.key()).into_iter().flatten().cloned());
        if let Some(by) = by {
            told.remove(by);
        }
        let mut pushes = self.pushes.lock().expect("pushes");
        for machine in told {
            pushes.push((machine, fastctx_types::assets::Push::Moved { scope: scope.into(), stream: stream.into(), next }));
        }
    }

    /// What is to be told unasked now, to the machines that read its scope
    /// in `directory` as it stands (design §6.4): one that no longer does
    /// is told nothing more of the scope until it reads it again.
    pub(crate) fn drain_pushes(&self) -> Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)> {
        std::mem::take(&mut *self.pushes.lock().expect("pushes"))
    }

    /// Of `pushes`, taken off before `directory` was read (so a revocation
    /// before any of them is in it), those to the machines that read their
    /// scopes.
    pub(crate) fn take_pushes(
        &self,
        pushes: Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)>,
        directory: &fastctx_net::directory_index::Indexed<'_>,
    ) -> Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)> {
        use fastctx_types::assets::Push;
        let mut judged: BTreeMap<(String, fastctx_types::MachineName), bool> = BTreeMap::new();
        let mut revoked = Vec::new();
        let mut told: Vec<_> = pushes
            .into_iter()
            .filter(|(machine, push)| {
                let key = match push {
                    Push::Moved { scope, .. } | Push::Fact { scope, .. } | Push::Revoked { scope } => scope.clone(),
                    Push::Checkpointed { checkpoint } => checkpoint.scope.clone(),
                };
                if matches!(push, Push::Revoked { .. }) {
                    return true;
                }
                let first = !judged.contains_key(&(key.clone(), machine.clone()));
                let reads = *judged.entry((key.clone(), machine.clone())).or_insert_with(|| {
                    matches!(scopes::scope(self, &key), Ok(Some(scope)) if scopes::readable(directory, machine, &scope))
                });
                if !reads {
                    // Told instead that it reads the scope no more, once —
                    // one that only follows its library never read it.
                    let held = self.holders.lock().expect("holders").get(&key).is_some_and(|machines| machines.contains(machine));
                    self.gone(&key, machine);
                    if let Some(machines) = self.receipted.lock().expect("receipted").get_mut(&key) {
                        machines.remove(machine);
                    }
                    if first && held {
                        revoked.push((machine.clone(), Push::Revoked { scope: key }));
                    }
                }
                reads
            })
            .collect();
        told.extend(revoked);
        told
    }

    /// `machine` is gone: it is told nothing of `scope` until it reads it
    /// again.
    pub(crate) fn gone(&self, scope: &str, machine: &fastctx_types::MachineName) {
        if let Some(machines) = self.holders.lock().expect("holders").get_mut(scope) {
            machines.remove(machine);
        }
    }

    /// One checkpoint of the scopes that changed since the last (design
    /// §6.1), at most `CHECKPOINT_AT_ONCE` of them: their heads go into the
    /// chain in one commit, then into the witness; the chain's number and
    /// head then, if anything changed. What fails goes to the next round.
    /// Call on a thread that may block.
    pub(crate) fn checkpoint(&self) -> io::Result<Option<(u64, [u8; 32])>> {
        if self.sealed().is_some() {
            return Ok(None);
        }
        let batch = {
            let mut changed = self.changed.lock().expect("changed scopes");
            let mut turn = self.turn.lock().expect("checkpoint turn");
            // The owners after the one the last round stopped at first.
            let order: Vec<Owner> = match turn.as_ref() {
                Some(last) => changed
                    .range((std::ops::Bound::Excluded(last.clone()), std::ops::Bound::Unbounded))
                    .chain(changed.range(..=last.clone()))
                    .map(|(owner, _)| owner.clone())
                    .collect(),
                None => changed.keys().cloned().collect(),
            };
            let mut batch: BTreeMap<Owner, BTreeSet<String>> = BTreeMap::new();
            let mut taken = 0;
            let mut scope_turns = self.scope_turns.lock().expect("scope turns");
            for owner in order {
                if taken >= CHECKPOINT_AT_ONCE {
                    break;
                }
                let Some(scopes) = changed.get_mut(&owner) else { continue };
                // Within the owner too, the scopes after the one the last
                // round stopped at first.
                let after: Vec<String> = match scope_turns.get(&owner) {
                    Some(last) => scopes
                        .range::<String, _>((std::ops::Bound::Excluded(last.clone()), std::ops::Bound::Unbounded))
                        .chain(scopes.range::<String, _>(..=last.clone()))
                        .take(CHECKPOINT_AT_ONCE - taken)
                        .cloned()
                        .collect(),
                    None => scopes.iter().take(CHECKPOINT_AT_ONCE - taken).cloned().collect(),
                };
                for scope in after {
                    scopes.remove(&scope);
                    scope_turns.insert(owner.clone(), scope.clone());
                    batch.entry(owner.clone()).or_default().insert(scope);
                    taken += 1;
                }
                if scopes.is_empty() {
                    changed.remove(&owner);
                }
                *turn = Some(owner);
            }
            batch
        };
        let done = {
            let _one = self.checkpointing.lock().expect("checkpointing");
            // A witness write that failed after its commit is made good
            // first, under the lock: no checkpoint is taken as kept while the
            // witness holds less (AM02).
            self.catch_up().and_then(|()| if batch.is_empty() { Ok(None) } else { self.checkpoint_of(&batch) })
        };
        if done.is_err() {
            let mut again = self.changed.lock().expect("changed scopes");
            for (owner, scopes) in batch {
                again.entry(owner).or_default().extend(scopes);
            }
        }
        done
    }

    /// A checkpoint of `scope` of `owner` now, as a window of it is given
    /// (II08): what the window shows is in the hub's history outside its
    /// directory before any device holds it, so a library put back to before
    /// it is found out as it opens. Nothing while sealed. Call on a thread
    /// that may block.
    pub(crate) fn checkpoint_now(&self, owner: &Owner, scope: &str) -> io::Result<()> {
        if self.sealed().is_some() {
            return Ok(());
        }
        let batch = BTreeMap::from([(owner.clone(), BTreeSet::from([scope.to_string()]))]);
        let _one = self.checkpointing.lock().expect("checkpointing");
        // Made good under the lock: a checkpoint another round wrote while
        // this waited, its witness write failed, is in the witness before
        // this one is found to need none (AM02).
        self.catch_up()?;
        self.checkpoint_of(&batch).map(|_| ())
    }

    fn checkpoint_of(&self, batch: &BTreeMap<Owner, BTreeSet<String>>) -> io::Result<Option<(u64, [u8; 32])>> {
        let mut owners: Vec<(String, Vec<Head>)> = Vec::new();
        let mut failed: Option<io::Error> = None;
        for (owner, scopes) in batch {
            let heads = (|| {
                let library = self.library(owner)?;
                // One deleted since it changed has no head to keep.
                let wanted: Vec<String> = scopes.iter().filter(|scope| !self.was_deleted(scope)).cloned().collect();
                let heads = library.heads(&wanted).map_err(|error| io::Error::other(error.to_string()))?;
                // A scope still where its latest checkpoint has it, or where
                // nothing is, needs none.
                let kept = self.checkpointed_of(owner, &wanted)?;
                Ok::<_, io::Error>(
                    heads
                        .into_iter()
                        .filter(|head| kept.get(&head.scope) != Some(head) && (head.change > 0 || head.parked > 0))
                        .collect::<Vec<Head>>(),
                )
            })();
            match heads {
                Ok(heads) if heads.is_empty() => {}
                Ok(heads) => owners.push((owner.key(), heads)),
                // An owner that fails holds up none of the others: it goes
                // to the next round alone.
                Err(error) => {
                    self.changed.lock().expect("changed scopes").entry(owner.clone()).or_default().extend(scopes.iter().cloned());
                    failed.get_or_insert(error);
                }
            }
        }
        if owners.is_empty() {
            return failed.map_or(Ok(None), Err);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"fastctx-hub-checkpoint/1");
        for (owner, heads) in &owners {
            hasher.update((owner.len() as u32).to_le_bytes());
            hasher.update(owner.as_bytes());
            for head in heads {
                hasher.update((head.scope.len() as u32).to_le_bytes());
                hasher.update(head.scope.as_bytes());
                hasher.update(head.change.to_le_bytes());
                hasher.update(head.prefix);
                hasher.update(head.parked.to_le_bytes());
                hasher.update(head.accumulated);
            }
        }
        let digest: [u8; 32] = hasher.finalize().into();
        let rows: Vec<(String, Head)> =
            owners.iter().flat_map(|(owner, heads)| heads.iter().map(|head| (owner.clone(), head.clone()))).collect();
        let sealing = self.sealing.clone();
        let written = self.control.write_now(256 + rows.len() * 64, move |c| {
            // Sealed meanwhile, nothing more goes into the chain (AM03).
            unsealed(&sealing)?;
            let (seq, head) = chain::append(c, "checkpoint", &digest)?;
            let mut keeping = c.prepare_cached(
                "INSERT OR REPLACE INTO hub_checkpoints (owner, scope, seq, change, parked, prefix, accumulated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (owner, head) in &rows {
                keeping.execute(fastctx_storage::rusqlite::params![
                    owner,
                    head.scope,
                    seq as i64,
                    head.change as i64,
                    head.parked as i64,
                    &head.prefix[..],
                    &head.accumulated[..]
                ])?;
            }
            Ok((seq, head))
        });
        let (seq, head) = written.map_err(io::Error::other)?;
        self.witness.advanced(seq, &head)?;
        // Frozen as the hub sealed, the witness holds less than the chain: no
        // device is told a receipt is covered by what the witness does not
        // hold, and lets go of it (AM03).
        if self.witness.current().0 < seq {
            return Err(io::Error::other("the witness was frozen as the checkpoint was written"));
        }
        // Who was given a receipt of a scope it covers hears so.
        let mut receipted = self.receipted.lock().expect("receipted");
        let mut pushes = self.pushes.lock().expect("pushes");
        let signer = self.signer();
        for (_, heads) in &owners {
            for covered in heads {
                let Some(machines) = receipted.remove(&covered.scope) else { continue };
                let statement =
                    fastctx_types::assets::checkpoint_statement(&covered.scope, covered.change, covered.parked, seq, &head);
                let checkpoint = fastctx_types::assets::Checkpoint {
                    scope: covered.scope.clone(),
                    change: covered.change,
                    parked: covered.parked,
                    seq,
                    head: head.into(),
                    word: signer.sign(&statement).into(),
                };
                for machine in machines {
                    pushes.push((machine, fastctx_types::assets::Push::Checkpointed { checkpoint: checkpoint.clone() }));
                }
            }
        }
        Ok(Some((seq, head)))
    }
}

/// Checkpoints the owner libraries every second, and closes the idle ones;
/// every ten minutes deletes the libraries no one has read for a week.
pub(crate) async fn checkpoints(hub: Weak<crate::Hub>) {
    let mut rounds = 0u32;
    let sweeping = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // What the operator asks of the vouched objects, looked for every minute,
    // one round at a time (`reclaim`).
    let reclaiming = hub.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let Some(running) = reclaiming.upgrade() else { return };
            let (assets, store, log) = (running.assets.clone(), running.store(), running.log.clone());
            drop(running);
            let hub = reclaiming.clone();
            let enter = move || hub.upgrade().and_then(|hub| crate::moving::Moving::enter(&hub));
            match fastctx_platform::task::spawn_blocking(move || assets.reclaim_round(&store.indexed(), &enter)).await {
                Ok(Ok(None)) => {}
                Ok(Ok(Some(said))) => log.line(format_args!("{said}")),
                Ok(Err(error)) => log.line(format_args!("letting go vouched objects: {error}")),
                Err(error) => log.line(format_args!("letting go vouched objects: {error}")),
            }
        }
    });
    // What the hub's records hold goes into the asset library once after it
    // starts (design §28.17).
    if let Some(running) = hub.upgrade() {
        fastctx_platform::task::spawn(mirror::everything(running));
    }
    // And its own log, as it grows (design §14).
    fastctx_platform::task::spawn(hub_log::follow(hub.clone()));
    // What failed to be written is tried again every minute (AR15).
    let again = hub.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let Some(running) = again.upgrade() else { return };
            mirror::again(running).await;
        }
    });
    loop {
        tokio::time::sleep(CHECKPOINT_EVERY).await;
        rounds = rounds.wrapping_add(1);
        // And what they hold of each scope described since, into it.
        if let Some(running) = hub.upgrade() {
            let described = std::mem::take(&mut *running.assets.described.lock().expect("described scopes"));
            if !described.is_empty() {
                fastctx_platform::task::spawn(mirror::described(running, described));
            }
        }
        // The sweep goes on beside the checkpoints, one at a time: a long one
        // does not hold them back (AC02).
        if rounds.is_multiple_of(SWEEP_EVERY)
            && !sweeping.swap(true, std::sync::atomic::Ordering::AcqRel)
            && let Some(running) = hub.upgrade()
        {
            let (assets, store, log) = (running.assets.clone(), running.store(), running.log.clone());
            drop(running);
            let sweeping = sweeping.clone();
            fastctx_platform::task::spawn(async move {
                let swept = fastctx_platform::task::spawn_blocking(move || assets.sweep_owners(&store.indexed())).await;
                sweeping.store(false, std::sync::atomic::Ordering::Release);
                match swept {
                    Ok(Ok(0)) => {}
                    Ok(Ok(dropped)) => log.line(format_args!("deleted {dropped} owner librar(ies) no one has read for a week")),
                    Ok(Err(error)) => log.line(format_args!("looking for libraries no one reads: {error}")),
                    Err(error) => log.line(format_args!("looking for libraries no one reads: {error}")),
                }
            });
        }
        let assets_hub = hub.clone();
        let Some(hub) = hub.upgrade() else { return };
        let assets = hub.assets.clone();
        let log = hub.log.clone();
        drop(hub);
        let done = fastctx_platform::task::spawn_blocking(move || {
            assets.owners.sweep();
            let checkpointed = assets.checkpoint();
            assets.let_go();
            checkpointed
        })
        .await;
        if let Some(hub) = assets_hub.upgrade() {
            let drained = hub.assets.drain_pushes();
            let store = hub.store();
            let pushes = hub.assets.take_pushes(drained, &store.indexed());
            drop(store);
            for (machine, push) in pushes {
                hub.tell(&machine, fastctx_net::wire::Down::Assets { push });
            }
        }
        match done {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log.line(format_args!("checkpointing the owner libraries: {error}")),
            Err(error) => log.line(format_args!("checkpointing the owner libraries: {error}")),
        }
    }
}
