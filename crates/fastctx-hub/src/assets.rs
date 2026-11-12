//! The hub's side of the second version's asset sync (design §6, §17): its
//! service key, its own history as a chain whose head is kept outside its
//! directory, its owner libraries with each second's checkpoint of them,
//! and the seal it keeps over every asset when it finds, on opening, that
//! it was put back from an older copy.

pub(crate) mod chain;
mod keys;
pub mod recovery;
pub(crate) mod objects;
mod owners;
mod scopes;
pub(crate) mod serve;
mod witness;

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

/// The asset side's tables in the hub's control database.
pub(crate) fn schema() -> String {
    format!("{}{}", chain::SCHEMA, scopes::SCHEMA)
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

/// How often the owner libraries are checkpointed.
pub(crate) const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);

/// Scopes checkpointed in one round: a backlog goes in pieces, each within
/// a commit's budget, the rest in the rounds after.
const CHECKPOINT_AT_ONCE: usize = 4096;

pub(crate) struct Assets {
    /// The hub's id, which each descriptor names (design §2.1): a scope of
    /// one hub is no other's.
    pub(crate) id: String,
    keys: Arc<keys::Keys>,
    pub(crate) owners: owners::Owners,
    pub(crate) witness: Arc<Witness>,
    pub(crate) control: Database,
    /// Why every asset is sealed, when the hub found it was put back
    /// (design §17): nothing is read or taken until recovery ends.
    sealed: Mutex<Option<String>>,
    /// The scopes of each owner library changed since the last checkpoint.
    changed: Mutex<BTreeMap<Owner, BTreeSet<String>>>,
    /// The machines that read each scope lately: told when a stream of it
    /// moves (design §6.6).
    holders: Mutex<BTreeMap<String, BTreeSet<fastctx_types::MachineName>>>,
    /// The machines given a receipt of each scope since the last
    /// checkpoint: told when a checkpoint covers it (design §17).
    receipted: Mutex<BTreeMap<String, BTreeSet<fastctx_types::MachineName>>>,
    /// What is to be told unasked, by whom answers a request.
    pushes: Mutex<Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)>>,
    /// The tickets for objects' data connections, and the proofs of
    /// holding that stand (design §6.5, §7.3).
    pub(crate) objects: objects::Objects,
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
        let owners = owners::Owners::new(dir.join("assets"), incarnation);
        let assets = Self {
            id: id.to_string(),
            keys,
            owners,
            witness,
            control,
            sealed: Mutex::new(None),
            changed: Mutex::new(BTreeMap::new()),
            holders: Mutex::new(BTreeMap::new()),
            receipted: Mutex::new(BTreeMap::new()),
            pushes: Mutex::new(Vec::new()),
            objects: objects::Objects::default(),
        };
        if let Some(why) = assets.put_back(dir)? {
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

    /// The chain's latest head reaches the witness, if it had not.
    fn catch_up(&self) -> io::Result<()> {
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
        match self.owners.library(owner, |library| self.behind(owner, library))? {
            Ok(library) => Ok(library),
            Err(why) => {
                self.seal(&why);
                Err(io::Error::other(why))
            }
        }
    }

    /// The latest head each scope of `owner` was checkpointed at.
    pub(crate) fn checkpointed(&self, owner: &Owner) -> io::Result<BTreeMap<String, Head>> {
        let key = owner.key();
        self.control
            .read_now(move |c| {
                let mut statement = c.prepare_cached(
                    "SELECT scope, change, parked, prefix, accumulated FROM hub_checkpoints h
                     WHERE owner = ?1 AND seq = (SELECT MAX(seq) FROM hub_checkpoints l WHERE l.owner = h.owner AND l.scope = h.scope)",
                )?;
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
        Ok(None)
    }

    pub(crate) fn keys(&self) -> &keys::Keys {
        &self.keys
    }

    /// Whether a chain head at `seq` that is not the chain's is of a history
    /// put aside when the hub recovered (design §17).
    pub(crate) fn closed_head(&self, seq: u64) -> io::Result<bool> {
        self.control
            .read_now(move |c| {
                Ok(c.prepare_cached("SELECT 1 FROM closed_branches WHERE fork < ?1 AND top >= ?1 LIMIT 1")?
                    .query_row([seq as i64], |_| Ok(()))
                    .optional()?
                    .is_some())
            })
            .map_err(io::Error::other)
    }

    /// Whether a receipt naming `head`, which the library does not hold, is of
    /// a history put aside when the hub recovered: past where its scope stood
    /// then.
    pub(crate) fn closed_receipt(&self, head: &Head) -> io::Result<bool> {
        let (scope, change, parked) = (head.scope.clone(), head.change as i64, head.parked as i64);
        self.control
            .read_now(move |c| {
                Ok(c.prepare_cached(
                    "SELECT 1 FROM closed_scopes WHERE scope = ?1 AND ((?2 > 0 AND ?2 > change) OR (?3 > 0 AND ?3 > parked)) LIMIT 1",
                )?
                .query_row(fastctx_storage::rusqlite::params![scope, change, parked], |_| Ok(()))
                .optional()?
                .is_some())
            })
            .map_err(io::Error::other)
    }

    /// What `machine` brought while every asset was sealed: kept for the
    /// recovery's report (design §17).
    pub(crate) fn brought(&self, machine: &fastctx_types::MachineName, seq: u64, receipts: usize) {
        let (name, at) = (machine.to_string(), jiff::Timestamp::now().as_millisecond());
        let _ = self.control.write_now(256, move |c| {
            c.prepare_cached(
                "INSERT INTO recovery_seen (machine, seq, receipts, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (machine) DO UPDATE SET seq = MAX(seq, excluded.seq), receipts = MAX(receipts, excluded.receipts), at = excluded.at",
            )?
            .execute(fastctx_storage::rusqlite::params![name, seq as i64, receipts as i64, at])?;
            Ok(())
        });
    }

    /// `machine` opened an asset session: it speaks this asset sync, and so
    /// clears for a share that waits for it (design §6.3).
    pub(crate) fn speaks(&self, machine: &fastctx_types::MachineName) -> io::Result<()> {
        let name = machine.to_string();
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

    /// Whether `machine` ever opened an asset session here.
    pub(crate) fn spoken(&self, machine: &fastctx_types::MachineName) -> bool {
        let name = machine.to_string();
        self.control
            .read_now(move |c| Ok(c.prepare_cached("SELECT 1 FROM asset_machines WHERE machine = ?1")?.query_row([&name], |_| Ok(())).optional()?))
            .is_ok_and(|found| found.is_some())
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
        self.witness.freeze();
        let mut sealed = self.sealed.lock().expect("seal");
        if sealed.is_none() {
            *sealed = Some(why.to_string());
        }
    }

    /// Why every asset is sealed, if it is.
    pub(crate) fn sealed(&self) -> Option<String> {
        self.sealed.lock().expect("seal").clone()
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

    /// `stream` of `scope` moved to `next`: each machine that reads the
    /// scope but `by` is to be told.
    pub(crate) fn moved(&self, scope: &str, stream: &str, next: u64, by: &fastctx_types::MachineName) {
        let told: Vec<_> = self.holders.lock().expect("holders").get(scope).into_iter().flatten().filter(|machine| *machine != by).cloned().collect();
        let mut pushes = self.pushes.lock().expect("pushes");
        for machine in told {
            pushes.push((machine, fastctx_types::assets::Push::Moved { scope: scope.into(), stream: stream.into(), next }));
        }
    }

    /// What is to be told unasked now, to the machines that read its scope
    /// in `directory` as it stands (design §6.4): one that no longer does
    /// is told nothing more of the scope until it reads it again.
    pub(crate) fn take_pushes(
        &self,
        directory: &fastctx_net::directory_index::Indexed<'_>,
    ) -> Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)> {
        use fastctx_types::assets::Push;
        let pushes = std::mem::take(&mut *self.pushes.lock().expect("pushes"));
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
                    self.gone(&key, machine);
                    if let Some(machines) = self.receipted.lock().expect("receipted").get_mut(&key) {
                        machines.remove(machine);
                    }
                    // Told instead that it reads the scope no more, once.
                    if first {
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
        // A witness write that failed after its commit is made good first.
        self.catch_up()?;
        let batch = {
            let mut changed = self.changed.lock().expect("changed scopes");
            let mut batch: BTreeMap<Owner, BTreeSet<String>> = BTreeMap::new();
            let mut taken = 0;
            while taken < CHECKPOINT_AT_ONCE {
                let Some(mut entry) = changed.first_entry() else { break };
                let owner = entry.key().clone();
                while taken < CHECKPOINT_AT_ONCE {
                    let Some(scope) = entry.get_mut().pop_first() else { break };
                    batch.entry(owner.clone()).or_default().insert(scope);
                    taken += 1;
                }
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
            batch
        };
        if batch.is_empty() {
            return Ok(None);
        }
        let done = self.checkpoint_of(&batch);
        if done.is_err() {
            let mut again = self.changed.lock().expect("changed scopes");
            for (owner, scopes) in batch {
                again.entry(owner).or_default().extend(scopes);
            }
        }
        done
    }

    fn checkpoint_of(&self, batch: &BTreeMap<Owner, BTreeSet<String>>) -> io::Result<Option<(u64, [u8; 32])>> {
        let mut owners: Vec<(String, Vec<Head>)> = Vec::new();
        for (owner, scopes) in batch {
            let library = self.library(owner)?;
            let scopes: Vec<String> = scopes.iter().cloned().collect();
            let heads = library.heads(&scopes).map_err(|error| io::Error::other(error.to_string()))?;
            // A scope still where its latest checkpoint has it, or where
            // nothing is, needs none.
            let kept = self.checkpointed(owner)?;
            let heads: Vec<Head> = heads
                .into_iter()
                .filter(|head| kept.get(&head.scope) != Some(head) && (head.change > 0 || head.parked > 0))
                .collect();
            if !heads.is_empty() {
                owners.push((owner.key(), heads));
            }
        }
        if owners.is_empty() {
            return Ok(None);
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
        let written = self.control.write_now(256 + rows.len() * 64, move |c| {
            let (seq, head) = chain::append(c, "checkpoint", &digest)?;
            let mut keeping = c.prepare_cached(
                "INSERT INTO hub_checkpoints (owner, scope, seq, change, parked, prefix, accumulated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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

/// Checkpoints the owner libraries every second, and closes the idle ones.
pub(crate) async fn checkpoints(hub: Weak<crate::Hub>) {
    loop {
        tokio::time::sleep(CHECKPOINT_EVERY).await;
        let assets_hub = hub.clone();
        let Some(hub) = hub.upgrade() else { return };
        let assets = hub.assets.clone();
        let log = hub.log.clone();
        drop(hub);
        let done = fastctx_platform::task::spawn_blocking(move || {
            assets.owners.sweep();
            assets.checkpoint()
        })
        .await;
        if let Some(hub) = assets_hub.upgrade() {
            let store = hub.store();
            let pushes = hub.assets.take_pushes(&store.indexed());
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
