//! What the hub keeps on disk.
//!
//! ```text
//! <hub>/hub.json              the hub's id and short name, its addresses, where it listens
//! <hub>/tls/certificate.der   the hub's certificate, pinned by every member
//! <hub>/tls/key.der
//! <hub>/login.secret          the hub's OPAQUE secrets (`fastctx_net::login::HubSecrets`)
//! <hub>/directory.json        accounts and their password records, members, shares, assistance,
//!                             acting, projects, enrollment tokens, removed members
//!                             and 90 days of machine-layer change records
//! <hub>/letters.json          where each member's letters stand
//! <hub>/outbox/<name>/<seq>   letters waiting for a member, one file each
//! <hub>/ledger/<town>/<seq>.json   the projects' ledgers (`crate::ledger`)
//! <hub>/moves.json            the moves that brought the hub here (`crate::moving`)
//! <hub>/moving-out.json       a move of the hub away from here, while and after it goes
//! <hub>/called-off.json       moves called off whose new place may still hold the hub
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
    /// The network was dissolved: the hub lets nobody in again, and tells
    /// every machine that was on it, as it comes, that it is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dissolved: Option<Dissolved>,
    /// The channel the network's devices follow and the version it asks
    /// for at least (`fastctx_types::update::Policy`).
    #[serde(default)]
    pub updates: fastctx_types::update::Policy,
}

/// Who dissolved the network, and when; and the machines and devices that
/// were on it and have not been told yet (`crate::dissolve`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dissolved {
    pub by: AccountName,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untold: Vec<PublicKey>,
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
    /// Conversations being deleted from the members keeping something of
    /// them (`crate::forget`).
    #[serde(default)]
    pub(crate) forgetting: crate::forget::Forgettings,
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
    /// The computers that use it become managed machines of the account
    /// (`fastctx_net::directory::Member::managed`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// How many devices may use it, any number at zero; a record from
    /// before codes for several devices says none and is for one.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub uses: u32,
    /// What its maker called it, as 「机房 A」.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Taken back: no device comes in by it any more.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub revoked: bool,
    pub made_by: MachineName,
    pub made_at: Timestamp,
    /// The key of the first device that used it, which may use it again to
    /// the same end, as when its first try was cut off.
    pub used_by: Option<PublicKey>,
    /// Every device that used it, the first among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used: Vec<PublicKey>,
    /// How it is given, only to be told (`fastctx_types::accounts::TokenWay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub way: Option<fastctx_types::accounts::TokenWay>,
}

/// The most devices one code lets in, short of any number.
pub const MOST_USES: u32 = 9999;

/// How long a code no device comes in by any more is kept before it is
/// forgotten: its maker still sees which machines came in by it, and a
/// device cut off on its last use may still finish.
const SPENT_KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(30 * 24);

fn one() -> u32 {
    1
}

fn is_one(uses: &u32) -> bool {
    *uses == 1
}

impl TokenRecord {
    /// Whether `key` used it already.
    pub fn used_by(&self, key: &PublicKey) -> bool {
        self.used_by.as_ref() == Some(key) || self.used.contains(key)
    }

    /// How many devices used it.
    pub fn used_count(&self) -> u32 {
        let mut count = self.used.len();
        if self.used_by.is_some_and(|first| !self.used.contains(&first)) {
            count += 1;
        }
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Why the device `key` may not come in by it now, if it may not. One
    /// that came in by it and is `inside` the hub still may come again, as
    /// when its first try was cut off, unless the code was revoked; any
    /// other, a device taken off the hub since among them, only while the
    /// code is open.
    pub fn refuses(&self, key: &PublicKey, inside: bool, now: Timestamp) -> Option<&'static str> {
        if self.revoked {
            return Some("the code was revoked");
        }
        if inside && self.used_by(key) {
            return None;
        }
        self.closed(now)
    }

    /// Whether no device has come in by it for long enough that it is
    /// forgotten: expired a month ago, or revoked or used up and made more
    /// than a month ago.
    pub fn spent(&self, now: Timestamp) -> bool {
        let Ok(month_ago) = now.checked_sub(SPENT_KEPT) else { return false };
        let used_up = self.uses != 0 && self.used_count() >= self.uses;
        self.expires < month_ago || ((self.revoked || used_up) && self.made_at < month_ago)
    }

    /// Why a device that did not use it yet may not, if it may not.
    pub fn closed(&self, now: Timestamp) -> Option<&'static str> {
        if self.revoked {
            Some("the code was revoked")
        } else if self.expires <= now {
            Some("the code has expired")
        } else if self.uses != 0 && self.used_count() >= self.uses {
            Some(if self.uses == 1 {
                "the token was used already"
            } else {
                "the code has been used as many times as it may"
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Removed {
    pub key: PublicKey,
    pub name: MachineName,
    pub by: Option<AccountName>,
    pub at: Timestamp,
    pub reason: String,
    /// It went with the whole network, dissolved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dissolved: bool,
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
    /// The member's last session said it takes a new name the hub gives it.
    #[serde(default)]
    pub renames: bool,
    /// The member's last session said it deletes what it keeps of a
    /// conversation when told, and reads the records of that.
    #[serde(default)]
    pub forgets: bool,
    /// The member's last session said it reads visitors among the
    /// accounts (`fastctx_net::wire::Intent::Session`).
    #[serde(default)]
    pub roles: bool,
    /// The member's last session said it reads the network's versions,
    /// and the records of the update policy with them.
    #[serde(default)]
    pub versions: bool,
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
    /// A move of the hub away from here (`crate::moving`).
    pub outgoing: Option<crate::moving::Outgoing>,
    /// The moves that brought the hub here, oldest first.
    pub moves: Vec<crate::moving::Incoming>,
    /// Moves called off whose new place may still hold the hub's identity,
    /// until it gives it up (`crate::moving::out`).
    pub called_off: Vec<crate::moving::Outgoing>,
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
        let outgoing = match read(&dir.join(crate::moving::OUTGOING)) {
            Ok(outgoing) => Some(outgoing),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let moves = read_or_default(&dir.join(crate::moving::MOVES))?;
        let called_off = read_or_default(&dir.join(crate::moving::CALLED_OFF))?;
        let mut store =
            Self { dir: dir.to_path_buf(), hub, book, letters, worked, upgraded, outgoing, moves, called_off };
        if changed {
            store.save_letters()?;
        }
        // A hub stopped between keeping a rename and moving what it keeps
        // for the machine finishes that now.
        let moved = store.settle_names()?;
        store.upgraded.extend(moved);
        Ok(store)
    }

    /// Moves what is kept for a member under a name it had to its name now:
    /// its letters and where they stand, and the work it was called to do.
    /// A rename does this once the directory that says it is kept, and a
    /// hub stopped in between does it when it opens. Says what moved.
    pub fn settle_names(&mut self) -> io::Result<Vec<String>> {
        let directory = &self.book.directory;
        let mut said = Vec::new();
        let renamed: Vec<(MachineName, MachineName)> = self
            .letters
            .keys()
            .filter_map(|kept| {
                let member = directory.member(kept)?;
                (member.name != *kept || member.name.as_str() != kept.as_str())
                    .then(|| (kept.clone(), member.name.clone()))
            })
            .collect();
        for (old, new) in renamed {
            // The same name spelled anew keeps its records where they are.
            if old != new {
                if self.letters.contains_key(&new) {
                    said.push(format!(
                        "letters are kept for both {old} and {new}, which it was renamed to; left as they are"
                    ));
                    continue;
                }
                let from = old.entry_in(&self.dir.join("outbox"))?;
                if std::fs::exists(&from)? {
                    let to = new.entry_in(&self.dir.join("outbox"))?;
                    std::fs::rename(&from, &to)?;
                }
            }
            let entry = self.letters.remove(&old).expect("listed above");
            self.letters.insert(new.clone(), entry);
            said.push(format!("the letters for {old} are kept for {new}, as it is called now"));
        }
        let worked: std::collections::BTreeSet<(MachineName, fastctx_types::Address)> = self
            .worked
            .iter()
            .map(|(machine, resident)| {
                let resident = fastctx_types::Address {
                    machine: directory.canonical(&resident.machine).clone(),
                    resident: resident.resident.clone(),
                };
                (directory.canonical(machine).clone(), resident)
            })
            .collect();
        let worked_moved = worked.iter().zip(&self.worked).any(|(now, before)| {
            now.0.as_str() != before.0.as_str() || now.1.machine.as_str() != before.1.machine.as_str()
        }) || worked.len() != self.worked.len();
        if !said.is_empty() {
            self.save_letters()?;
        }
        if worked_moved {
            self.worked = worked;
            publish(&self.dir.join("worked.json"), &json(&self.worked)?, Permissions::Private)?;
        }
        Ok(said)
    }

    /// The directory as kept; one kept in the shape before sharing and
    /// assistance (`crate::upgrade`) is kept again in today's, after a copy of
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

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save_outgoing(&self) -> io::Result<()> {
        let path = self.dir.join(crate::moving::OUTGOING);
        match &self.outgoing {
            Some(outgoing) => publish(&path, &json(outgoing)?, Permissions::Private),
            None => match std::fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
        }
    }

    pub fn save_called_off(&self) -> io::Result<()> {
        publish(&self.dir.join(crate::moving::CALLED_OFF), &json(&self.called_off)?, Permissions::Private)
    }

    pub fn save_moves(&self) -> io::Result<()> {
        publish(&self.dir.join(crate::moving::MOVES), &json(&self.moves)?, Permissions::Private)
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
                renames: false,
                forgets: false,
                roles: false,
                versions: false,
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

    /// Drops the letters waiting for the conversation at `conversation`,
    /// being deleted; says how many. A part of a letter too large for one
    /// message names nobody, and stays.
    pub fn drop_letters_for(&mut self, conversation: &fastctx_types::Address) -> io::Result<usize> {
        use fastctx_types::remote::Letter;
        let mut dropped = 0;
        for (_, path) in outbox_entries(&self.dir, &conversation.machine)? {
            let message: Envelope = read(&path)?;
            let Ok(body) = message.body::<fastctx_net::message::LetterBody>() else { continue };
            let to = match &body.letter {
                Letter::JobFacts { to, .. } | Letter::Notice { to, .. } | Letter::Speak { to, .. } => to,
                Letter::Fragment { .. } | Letter::Left { .. } => continue,
            };
            if *to == conversation.resident {
                match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => dropped += 1,
                }
            }
        }
        Ok(dropped)
    }

    /// Forgets which machines were called to work for the conversation at
    /// `conversation`, deleted.
    pub fn forget_worked(&mut self, conversation: &fastctx_types::Address) -> io::Result<()> {
        let before = self.worked.len();
        self.worked.retain(|(_, worked)| worked != conversation);
        if before != self.worked.len() {
            publish(&self.dir.join("worked.json"), &json(&self.worked)?, Permissions::Private)?;
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

pub(crate) fn read_or_default<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match read(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        other => other,
    }
}
