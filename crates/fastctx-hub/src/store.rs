//! What the hub keeps on disk.
//!
//! ```text
//! <hub>/hub.json              the World's id, the hub's addresses, the founding secret until it is used
//! <hub>/tls/certificate.der   the hub's certificate, pinned by every machine
//! <hub>/tls/key.der
//! <hub>/members.json          every record, revocation and invitation
//! <hub>/letters.json          where each member's letters stand
//! <hub>/outbox/<name>/<seq>   letters waiting for a member, one file each
//! ```
//!
//! Every file is replaced whole, so a crash leaves the old state or the new
//! one. Letters are written before the numbers that count them; a hub that
//! stops in between finds the letter files and counts from the highest.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use fastctx_net::sign::{PublicKey, Secret, Signed};
use fastctx_net::trust::{self, Credential, Known, Record, Table, WorldId};
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The hub's settings, written when it is created.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubFile {
    pub world: WorldId,
    /// Where machines reach the hub, written into every code.
    pub public: Vec<SocketAddr>,
    /// Where the hub listens.
    pub listen: SocketAddr,
    /// Present until the first machine founds the World with it.
    pub founding: Option<Secret>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Members {
    pub records: Vec<Admitted>,
    pub revocations: Vec<Signed>,
    pub invitations: Vec<Invitation>,
    #[serde(default)]
    pub grants: Vec<Signed>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Admitted {
    pub name: MachineName,
    pub record: Signed,
    pub joined_at: Timestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invitation {
    pub endorsement: Signed,
    pub invite: PublicKey,
    pub inviter: MachineName,
    pub expires: Timestamp,
    pub used_by: Option<MachineName>,
}

/// Where one member's letters stand.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Letters {
    /// The member's numbering of its own letters, and the last of them the
    /// hub holds.
    pub epoch: u64,
    pub taken: u64,
    /// The hub's numbering of its letters to the member, and the number the
    /// next one gets.
    pub out_epoch: u64,
    pub next: u64,
    /// When the member was last connected; `None` when the hub stopped
    /// while it was connected, so when it left is not known.
    pub last_seen: Option<Timestamp>,
    /// Set while the member is connected.
    pub connected_at: Option<Timestamp>,
}

pub struct Store {
    dir: PathBuf,
    pub hub: HubFile,
    pub members: Members,
    pub letters: BTreeMap<MachineName, Letters>,
    /// The members as the hub's own list checks out.
    table: Table,
    founder: Option<PublicKey>,
}

impl Store {
    pub fn hub_file(dir: &Path) -> PathBuf {
        dir.join("hub.json")
    }

    pub fn certificate_file(dir: &Path) -> PathBuf {
        dir.join("tls").join("certificate.der")
    }

    pub fn key_file(dir: &Path) -> PathBuf {
        dir.join("tls").join("key.der")
    }

    /// Creates a hub's files in `dir`, which must not hold a hub already.
    pub fn create(dir: &Path, hub: &HubFile, certificate: &fastctx_net::tls::HubCertificate) -> io::Result<()> {
        ensure_private_dir(dir)?;
        ensure_private_dir(&dir.join("tls"))?;
        publish_new(&Self::hub_file(dir), &json(hub)?, Permissions::Private)?;
        publish_new(&Self::certificate_file(dir), &certificate.certificate, Permissions::Private)?;
        publish_new(&Self::key_file(dir), &certificate.key, Permissions::Private)?;
        Ok(())
    }

    pub fn open(dir: &Path) -> io::Result<Self> {
        let hub: HubFile = read(&Self::hub_file(dir))?;
        let members: Members = read_or_default(&dir.join("members.json"))?;
        let mut letters: BTreeMap<MachineName, Letters> = read_or_default(&dir.join("letters.json"))?;
        let mut changed = false;
        for (name, entry) in &mut letters {
            // A member connected when the hub stopped left at a time the
            // hub never saw.
            if entry.connected_at.take().is_some() {
                entry.last_seen = None;
                changed = true;
            }
            // A letter written just before a stop may not be counted yet.
            if let Some(highest) = outbox_entries(dir, name)?.last().map(|(seq, _)| *seq)
                && highest >= entry.next
            {
                entry.next = highest + 1;
                changed = true;
            }
        }
        let mut store = Self { dir: dir.to_path_buf(), hub, members, letters, table: Table::default(), founder: None };
        store.recheck();
        if changed {
            store.save_letters()?;
        }
        Ok(store)
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn founder(&self) -> Option<PublicKey> {
        self.founder
    }

    /// Checks the hub's own list the way every machine checks it.
    pub fn recheck(&mut self) {
        let records: Vec<Signed> = self.members.records.iter().map(|admitted| admitted.record.clone()).collect();
        self.founder = self.members.records.iter().find_map(|admitted| {
            let record = admitted.record.claim::<Record>().ok()?;
            matches!(record.credential, Credential::Founder).then_some(record.key)
        });
        self.table = match &self.founder {
            Some(founder) => {
                trust::check(
                    &self.hub.world,
                    founder,
                    &records,
                    &self.members.revocations,
                    &self.members.grants,
                    &Known::default(),
                )
                .0
            }
            None => Table::default(),
        };
    }

    pub fn save_hub(&self) -> io::Result<()> {
        publish(&Self::hub_file(&self.dir), &json(&self.hub)?, Permissions::Private)
    }

    pub fn save_members(&mut self) -> io::Result<()> {
        publish(&self.dir.join("members.json"), &json(&self.members)?, Permissions::Private)?;
        self.recheck();
        Ok(())
    }

    pub fn save_letters(&self) -> io::Result<()> {
        publish(&self.dir.join("letters.json"), &json(&self.letters)?, Permissions::Private)
    }

    /// The letter numbers of a new member, starting fresh.
    pub fn new_letters() -> io::Result<Letters> {
        Ok(Letters {
            epoch: 0,
            taken: 0,
            out_epoch: u64::from_le_bytes(fastctx_net::sign::random::<8>()?),
            next: 1,
            last_seen: None,
            connected_at: None,
        })
    }

    /// Keeps `message` for `to` and returns its number.
    pub fn keep_letter(&mut self, to: &MachineName, message: &Signed) -> io::Result<u64> {
        let entry = self.letters.get_mut(to).ok_or_else(|| io::Error::other(format!("no letter records for {to}")))?;
        let seq = entry.next;
        let directory = self.dir.join("outbox").join(to.as_str());
        ensure_private_dir(&directory)?;
        publish(&directory.join(format!("{seq:020}")), &json(message)?, Permissions::Private)?;
        entry.next = seq + 1;
        Ok(seq)
    }

    /// The letters waiting for `name` after `after`, in order.
    pub fn letters_after(&self, name: &MachineName, after: u64) -> io::Result<Vec<(u64, Signed)>> {
        let mut letters = Vec::new();
        for (seq, path) in outbox_entries(&self.dir, name)? {
            if seq > after {
                letters.push((seq, read(&path)?));
            }
        }
        Ok(letters)
    }

    /// Forgets the letters `name` has taken, through `through`.
    pub fn letters_taken(&self, name: &MachineName, through: u64) -> io::Result<()> {
        for (seq, path) in outbox_entries(&self.dir, name)? {
            if seq <= through {
                match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Drops every letter waiting for `name`, as when it is revoked.
    pub fn drop_letters(&self, name: &MachineName) -> io::Result<()> {
        match std::fs::remove_dir_all(self.dir.join("outbox").join(name.as_str())) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// When the latest invitation, grant or revocation held was made.
    pub fn latest(&self) -> Option<Timestamp> {
        let invitations = self.members.invitations.iter().filter_map(|invitation| {
            invitation.endorsement.claim::<fastctx_net::trust::Endorsement>().ok().and_then(|endorsement| endorsement.at)
        });
        let grants =
            self.members.grants.iter().filter_map(|signed| signed.claim::<fastctx_net::scope::Grant>().ok()).map(|grant| grant.at);
        let revocations = self
            .members
            .revocations
            .iter()
            .filter_map(|signed| signed.claim::<fastctx_net::trust::Revocation>().ok())
            .map(|revocation| revocation.at);
        invitations.chain(grants).chain(revocations).max()
    }

    pub fn invitation(&self, invite: &PublicKey) -> Option<&Invitation> {
        self.members.invitations.iter().find(|invitation| invitation.invite == *invite)
    }
}

/// The letter files waiting for `name`, by number.
fn outbox_entries(dir: &Path, name: &MachineName) -> io::Result<Vec<(u64, PathBuf)>> {
    let directory = dir.join("outbox").join(name.as_str());
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        // Temporary files of an interrupted write start with a dot.
        if let Some(seq) = path.file_name().and_then(|name| name.to_str()).and_then(|name| name.parse::<u64>().ok()) {
            found.push((seq, path));
        }
    }
    found.sort();
    Ok(found)
}

fn json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match read(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        other => other,
    }
}
