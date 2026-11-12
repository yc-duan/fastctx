//! The hub's side of the second version's asset sync (design §6, §17): its
//! service key, its own history as a chain whose head is kept outside its
//! directory, its owner libraries with each second's checkpoint of them,
//! and the seal it keeps over every asset when it finds, on opening, that
//! it was put back from an older copy.

pub(crate) mod chain;
mod keys;
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

pub(crate) struct Assets {
    keys: Arc<keys::Keys>,
    pub(crate) owners: owners::Owners,
    pub(crate) witness: Arc<Witness>,
    control: Database,
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
    /// items 1 and 2). Call on a thread that may block.
    pub(crate) fn open(dir: &Path, id: &str, control: Database) -> io::Result<Self> {
        let keys = Arc::new(keys::Keys::open(&dir.join("keys"))?);
        let witness = Arc::new(Witness::open(dir, id)?);
        let incarnation = incarnation(&control)?;
        let owners = owners::Owners::new(dir.join("assets"), incarnation);
        let assets = Self {
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
        }
        Ok(assets)
    }

    /// Why the hub was put back, if it was: its backup's mark is here; the
    /// witness outside its directory holds a chain head its database does
    /// not; an owner library no longer holds the heads its latest
    /// checkpoint wrote, outside the directory or in the hub's own database
    /// (a backup copied in part); or a library is another hub's.
    fn put_back(&self, dir: &Path) -> io::Result<Option<String>> {
        if fastctx_platform::disk::exists(dir.join(BACKUP_MARKER))? {
            return Ok(Some("the hub was opened from a backup".into()));
        }
        let kept = self.witness.kept();
        if kept.seq > 0 {
            let seq = kept.seq;
            let head = self.control.read_now(move |c| chain::head_at(c, seq)).map_err(io::Error::other)?;
            if head.map(|head| witness::hex(&head)) != Some(kept.head.clone()) {
                return Ok(Some(format!(
                    "the hub's directory is older than what it last wrote outside it ({})",
                    self.witness.path().display()
                )));
            }
        }
        for (key, heads) in &kept.owners {
            let Some(owner) = Owner::from_key(key) else { continue };
            if !self.owners.exists(&owner)? {
                return Ok(Some(format!("the library of {key} is gone")));
            }
            let library = self.owners.library(&owner)?;
            for kept in heads {
                let held = match kept.head() {
                    Some(head) => library.holds(&head).map_err(|error| io::Error::other(error.to_string()))?,
                    None => false,
                };
                if !held {
                    return Ok(Some(format!("the library of {key} is older than its last checkpoint")));
                }
            }
        }
        if let Some(why) = self.owners.foreign()? {
            return Ok(Some(why));
        }
        // The heads each library's latest checkpoint wrote in the hub's own
        // database: a library older than its database is a copy in part.
        let latest: Vec<(String, Head)> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached(
                    "SELECT owner, scope, change, parked, prefix, accumulated FROM hub_checkpoints h
                     WHERE seq = (SELECT MAX(seq) FROM hub_checkpoints l WHERE l.owner = h.owner AND l.scope = h.scope)",
                )?;
                let rows = statement.query_map([], |row| {
                    let digest = |bytes: Vec<u8>| <[u8; 32]>::try_from(bytes).unwrap_or([0; 32]);
                    Ok((
                        row.get::<_, String>(0)?,
                        Head {
                            scope: row.get(1)?,
                            change: row.get::<_, i64>(2)? as u64,
                            parked: row.get::<_, i64>(3)? as u64,
                            prefix: digest(row.get(4)?),
                            accumulated: digest(row.get(5)?),
                        },
                    ))
                })?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        for (key, head) in &latest {
            let Some(owner) = Owner::from_key(key) else { continue };
            if !self.owners.exists(&owner)? {
                return Ok(Some(format!("the library of {key} is gone")));
            }
            let library = self.owners.library(&owner)?;
            if !library.holds(head).map_err(|error| io::Error::other(error.to_string()))? {
                return Ok(Some(format!("the library of {key} is older than the hub's own record of it")));
            }
        }
        Ok(None)
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

    /// What is to be told unasked now; a machine that is not there to be
    /// told is told no more of the scope until it reads it again.
    pub(crate) fn take_pushes(&self) -> Vec<(fastctx_types::MachineName, fastctx_types::assets::Push)> {
        std::mem::take(&mut *self.pushes.lock().expect("pushes"))
    }

    /// `machine` is gone: it is told nothing of `scope` until it reads it
    /// again.
    pub(crate) fn gone(&self, scope: &str, machine: &fastctx_types::MachineName) {
        if let Some(machines) = self.holders.lock().expect("holders").get_mut(scope) {
            machines.remove(machine);
        }
    }

    /// One checkpoint of the scopes that changed since the last (design
    /// §6.1): their heads go into the chain in one commit, then into the
    /// witness; the chain's number and head then, if anything changed.
    /// Call on a thread that may block.
    pub(crate) fn checkpoint(&self) -> io::Result<Option<(u64, [u8; 32])>> {
        if self.sealed().is_some() {
            return Ok(None);
        }
        let changed = std::mem::take(&mut *self.changed.lock().expect("changed scopes"));
        if changed.is_empty() {
            return Ok(None);
        }
        let mut owners: Vec<(String, Vec<Head>)> = Vec::new();
        for (owner, scopes) in &changed {
            let library = self.owners.library(owner)?;
            let scopes: Vec<String> = scopes.iter().cloned().collect();
            let heads = library.heads(&scopes).map_err(|error| io::Error::other(error.to_string()))?;
            owners.push((owner.key(), heads));
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
        let (seq, head) = match written {
            Ok(written) => written,
            Err(error) => {
                // What changed is taken again by the next checkpoint.
                let mut again = self.changed.lock().expect("changed scopes");
                for (owner, scopes) in changed {
                    again.entry(owner).or_default().extend(scopes);
                }
                return Err(io::Error::other(error));
            }
        };
        self.witness.advanced(seq, &head, owners.clone())?;
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
            for (machine, push) in hub.assets.take_pushes() {
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
