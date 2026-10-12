//! What the hub keeps on disk.
//!
//! ```text
//! <hub>/hub.json              the hub's id and short name, its addresses, where it listens
//! <hub>/tls/certificate.der   the hub's certificate, pinned by every member
//! <hub>/tls/key.der
//! <hub>/login.secret          the hub's OPAQUE secrets (`fastctx_net::login::HubSecrets`)
//! <hub>/directory.json        accounts and their password records, members, shares, loans,
//!                             acting, projects, enrollment tokens, removed members
//!                             and 90 days of machine-layer change records
//! <hub>/letters.json          where each member's letters stand
//! <hub>/outbox/<name>/<seq>   letters waiting for a member, one file each
//! <hub>/ledger/<town>/<seq>.json   the projects' ledgers (`crate::ledger`)
//! ```
//!
//! Every file is replaced whole, so a crash leaves the old state or the new
//! one. Letters are written before the numbers that count them; a hub that
//! stops in between finds the letter files and counts from the highest.

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use fastctx_net::directory::{Directory, Kind, Project};
use fastctx_net::envelope::Envelope;
use fastctx_net::ids::HubId;
use fastctx_net::sign::{Bytes, PublicKey};
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::{AccountName, HubName, MachineName};
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The hub's settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubFile {
    pub id: HubId,
    pub name: HubName,
    /// Where machines reach the hub, written into every token.
    pub public: Vec<SocketAddr>,
    /// Where the hub listens.
    pub listen: SocketAddr,
    /// Whether anyone who reaches the hub may open an account.
    #[serde(default)]
    pub self_registration: bool,
}

/// The directory as the hub keeps it: what every member sees a cut of, and
/// what only the hub holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub(crate) machine_log: crate::machine_log::History,
    #[serde(flatten)]
    pub directory: Directory,
    /// What setting each account's password left with the hub.
    #[serde(default)]
    pub passwords: BTreeMap<AccountName, Bytes>,
    #[serde(default)]
    pub tokens: Vec<TokenRecord>,
    /// Members taken off the hub, by key, so one that connects again is
    /// told so.
    #[serde(default)]
    pub removed: Vec<Removed>,
    /// Projects of accounts that were deleted, kept whole for the super
    /// administrator to hand to another account.
    #[serde(default)]
    pub archived: Vec<Project>,
}

/// An enrollment token, by the public half of its key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenRecord {
    pub key: PublicKey,
    pub account: AccountName,
    pub register: bool,
    /// The name the device takes; its own choice when `None`.
    pub name: Option<MachineName>,
    pub kind: Kind,
    pub expires: Timestamp,
    pub made_by: MachineName,
    pub made_at: Timestamp,
    /// The key of the device that used it, which may use it again to the
    /// same end, as when its first try was cut off.
    pub used_by: Option<PublicKey>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Removed {
    pub key: PublicKey,
    pub name: MachineName,
    pub by: Option<AccountName>,
    pub at: Timestamp,
    pub reason: String,
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
    /// The member's last session said it reads names of any language
    /// (`fastctx_net::wire::Intent::Session`).
    #[serde(default)]
    pub any_names: bool,
}

pub struct Store {
    dir: PathBuf,
    pub hub: HubFile,
    pub book: Book,
    pub letters: BTreeMap<MachineName, Letters>,
    /// Which machine was called to work for which resident: what it tells
    /// that resident of the work goes on after the account's right on the
    /// machine is taken back (`world-network.md`, 收回立刻生效).
    pub worked: std::collections::BTreeSet<(MachineName, fastctx_types::Address)>,
    /// What reading a directory kept in an earlier shape into today's
    /// changed, when this opening did it (`crate::upgrade`).
    pub upgraded: Vec<String>,
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

    pub fn secrets_file(dir: &Path) -> PathBuf {
        dir.join("login.secret")
    }

    pub fn directory_file(dir: &Path) -> PathBuf {
        dir.join("directory.json")
    }

    /// Creates a hub's files in `dir`, which must not hold a hub already.
    pub fn create(
        dir: &Path,
        hub: &HubFile,
        certificate: &fastctx_net::tls::HubCertificate,
        secrets: &[u8],
        book: &Book,
    ) -> io::Result<()> {
        ensure_private_dir(dir)?;
        ensure_private_dir(&dir.join("tls"))?;
        publish_new(&Self::certificate_file(dir), &certificate.certificate, Permissions::Private)?;
        publish_new(&Self::key_file(dir), &certificate.key, Permissions::Private)?;
        publish_new(&Self::secrets_file(dir), secrets, Permissions::Private)?;
        publish_new(&Self::directory_file(dir), &json(book)?, Permissions::Private)?;
        // Last: a directory with `hub.json` is a hub.
        publish_new(&Self::hub_file(dir), &json(hub)?, Permissions::Private)
    }

    pub fn open(dir: &Path) -> io::Result<Self> {
        let hub: HubFile = read(&Self::hub_file(dir))?;
        let (book, upgraded) = Self::read_book(dir)?;
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
        let worked = read_or_default(&dir.join("worked.json"))?;
        let store = Self { dir: dir.to_path_buf(), hub, book, letters, worked, upgraded };
        if changed {
            store.save_letters()?;
        }
        Ok(store)
    }

    /// The directory as kept; one kept in the shape before sharing and
    /// lending (`crate::upgrade`) is kept again in today's, after a copy of
    /// it as it was is left beside it, which going back to the earlier
    /// fastctx needs. What changed is said too, and kept beside it. The
    /// relays a directory kept before direct connections replaced them are
    /// taken out, and the directory kept again without them.
    fn read_book(dir: &Path) -> io::Result<(Book, Vec<String>)> {
        let path = Self::directory_file(dir);
        let raw: serde_json::Value = read(&path)?;
        let (mut upgraded, mut said) = crate::upgrade::upgrade(raw);
        let shaped = !said.is_empty();
        let relays = crate::upgrade::drop_relays(&mut upgraded);
        let book: Book = serde_json::from_value(upgraded)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        if shaped {
            let before = dir.join("directory.before-sharing.json");
            if !before.exists() {
                publish_new(&before, &std::fs::read(&path)?, Permissions::Private)?;
            }
        }
        said.extend(relays);
        if !said.is_empty() {
            publish(&path, &json(&book)?, Permissions::Private)?;
            let mut notes = said.join("\n");
            notes.push('\n');
            publish(&dir.join("directory.upgraded.txt"), notes.as_bytes(), Permissions::Private)?;
        }
        Ok((book, said))
    }

    pub fn save_hub(&self) -> io::Result<()> {
        publish(&Self::hub_file(&self.dir), &json(&self.hub)?, Permissions::Private)
    }

    pub fn save_book(&self) -> io::Result<()> {
        publish(&Self::directory_file(&self.dir), &json(&self.book)?, Permissions::Private)
    }

    /// Notes that `machine` was called to work for `resident`.
    pub fn note_worked(&mut self, machine: &MachineName, resident: fastctx_types::Address) -> io::Result<()> {
        if self.worked.insert((machine.clone(), resident)) {
            publish(&self.dir.join("worked.json"), &json(&self.worked)?, Permissions::Private)?;
        }
        Ok(())
    }

    pub fn save_letters(&self) -> io::Result<()> {
        publish(&self.dir.join("letters.json"), &json(&self.letters)?, Permissions::Private)
    }

    /// The letter records of a member, made when it first connects.
    pub fn letters_of(&mut self, name: &MachineName) -> io::Result<&mut Letters> {
        if !self.letters.contains_key(name) {
            let fresh = Letters {
                epoch: 0,
                taken: 0,
                out_epoch: u64::from_le_bytes(fastctx_net::sign::random::<8>()?),
                next: 1,
                last_seen: None,
                connected_at: None,
                any_names: false,
            };
            self.letters.insert(name.clone(), fresh);
        }
        Ok(self.letters.get_mut(name).expect("just made"))
    }

    /// Keeps `message` for `to` and returns its number.
    pub fn keep_letter(&mut self, to: &MachineName, message: &Envelope) -> io::Result<u64> {
        let directory = to.entry_in(&self.dir.join("outbox"))?;
        let entry = self.letters_of(to)?;
        let seq = entry.next;
        ensure_private_dir(&directory)?;
        publish(&directory.join(format!("{seq:020}")), &json(message)?, Permissions::Private)?;
        entry.next = seq + 1;
        Ok(seq)
    }

    /// The letters waiting for `name` after `after`, in order.
    pub fn letters_after(&self, name: &MachineName, after: u64) -> io::Result<Vec<(u64, Envelope)>> {
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

    /// Drops every letter waiting for `name` and its letter records, as
    /// when it is taken off the hub.
    pub fn forget_letters(&mut self, name: &MachineName) -> io::Result<()> {
        self.letters.remove(name);
        match std::fs::remove_dir_all(name.entry_in(&self.dir.join("outbox"))?) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// The letter files waiting for `name`, by number.
fn outbox_entries(dir: &Path, name: &MachineName) -> io::Result<Vec<(u64, PathBuf)>> {
    let directory = name.entry_in(&dir.join("outbox"))?;
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

pub(crate) fn json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
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
