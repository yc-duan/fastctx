//! The hub's side of the second version's asset sync (design §6, §17): its
//! service key, its own history as a chain whose head is kept outside its
//! directory, its owner libraries with each second's checkpoint of them,
//! and the seal it keeps over every asset when it finds, on opening, that
//! it was put back from an older copy.

pub(crate) mod chain;
mod keys;
mod owners;
mod witness;

pub(crate) use owners::Owner;
pub(crate) use witness::Witness;

use fastctx_content::v2::{Head, HubSigner};
use fastctx_storage::Database;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// The mark a backup of the hub carries and a live hub's directory never
/// does (design §17, item 1): a hub opened with it here was put back.
pub(crate) const BACKUP_MARKER: &str = "backup-marker";

/// How often the owner libraries are checkpointed.
pub(crate) const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);

pub(crate) struct Assets {
    #[allow(dead_code)] // signs once the asset wire lands (3-1)
    keys: Arc<keys::Keys>,
    pub(crate) owners: owners::Owners,
    pub(crate) witness: Arc<Witness>,
    control: Database,
    /// Why every asset is sealed, when the hub found it was put back
    /// (design §17): nothing is read or taken until recovery ends.
    sealed: Mutex<Option<String>>,
    /// The scopes of each owner library changed since the last checkpoint.
    changed: Mutex<BTreeMap<Owner, BTreeSet<String>>>,
}

impl Assets {
    /// Opens the asset side of the hub `id` kept in `dir`, whose control
    /// database is `control`, and checks it was not put back (design §17,
    /// items 1 and 2). Call on a thread that may block.
    pub(crate) fn open(dir: &Path, id: &str, control: Database) -> io::Result<Self> {
        let keys = Arc::new(keys::Keys::open(&dir.join("keys"))?);
        let witness = Arc::new(Witness::open(dir, id)?);
        let owners = owners::Owners::new(dir.join("assets"));
        let assets =
            Self { keys, owners, witness, control, sealed: Mutex::new(None), changed: Mutex::new(BTreeMap::new()) };
        if let Some(why) = assets.put_back(dir)? {
            assets.witness.freeze();
            *assets.sealed.lock().expect("seal") = Some(why);
        }
        Ok(assets)
    }

    /// Why the hub was put back, if it was: its backup's mark is here; the
    /// witness outside its directory holds a chain head its database does
    /// not; or an owner library no longer holds the heads its latest
    /// checkpoint wrote.
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
        Ok(None)
    }

    /// Why every asset is sealed, if it is.
    pub(crate) fn sealed(&self) -> Option<String> {
        self.sealed.lock().expect("seal").clone()
    }

    /// The hub's signer of what it says of assets.
    #[allow(dead_code)] // used once the asset wire lands (3-1)
    pub(crate) fn signer(&self) -> Arc<dyn HubSigner> {
        Arc::new(keys::Signer { keys: self.keys.clone(), witness: self.witness.clone() })
    }

    #[allow(dead_code)] // used once the asset wire lands (3-1)
    pub(crate) fn public(&self) -> fastctx_net::sign::PublicKey {
        self.keys.public()
    }

    /// `scope` of `owner`'s library changed: the next checkpoint takes it.
    #[allow(dead_code)] // used once the asset wire lands (3-1)
    pub(crate) fn changed(&self, owner: Owner, scope: &str) {
        self.changed.lock().expect("changed scopes").entry(owner).or_default().insert(scope.to_string());
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
        let rows: Vec<(String, String, u64, u64)> = owners
            .iter()
            .flat_map(|(owner, heads)| heads.iter().map(|head| (owner.clone(), head.scope.clone(), head.change, head.parked)))
            .collect();
        let written = self.control.write_now(256 + rows.len() * 64, move |c| {
            let (seq, head) = chain::append(c, "checkpoint", &digest)?;
            let mut keeping = c.prepare_cached(
                "INSERT INTO hub_checkpoints (owner, scope, seq, change, parked) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (owner, scope, change, parked) in &rows {
                keeping.execute(fastctx_storage::rusqlite::params![owner, scope, seq as i64, *change as i64, *parked as i64])?;
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
        self.witness.advanced(seq, &head, owners)?;
        Ok(Some((seq, head)))
    }
}

/// Checkpoints the owner libraries every second, and closes the idle ones.
pub(crate) async fn checkpoints(hub: Weak<crate::Hub>) {
    loop {
        tokio::time::sleep(CHECKPOINT_EVERY).await;
        let Some(hub) = hub.upgrade() else { return };
        let assets = hub.assets.clone();
        let log = hub.log.clone();
        drop(hub);
        let done = fastctx_platform::task::spawn_blocking(move || {
            assets.owners.sweep();
            assets.checkpoint()
        })
        .await;
        match done {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log.line(format_args!("checkpointing the owner libraries: {error}")),
            Err(error) => log.line(format_args!("checkpointing the owner libraries: {error}")),
        }
    }
}
