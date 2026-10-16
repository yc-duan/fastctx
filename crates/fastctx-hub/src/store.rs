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
//! <hub>/outbox-trash/         the letters of members taken off, set aside to be deleted
//! <hub>/taken/<name>          a member's count of its own letters, the last of which the hub holds
//! <hub>/ledger/<town>/<seq>.json   the projects' ledgers (`crate::ledger`)
//! <hub>/moves.json            the moves that brought the hub here (`crate::moving`)
//! <hub>/moving-out.json       a move of the hub away from here, while and after it goes
//! <hub>/called-off.json       moves called off whose new place may still hold the hub
//! ```
//!
//! Every file is replaced whole, so a crash leaves the old state or the new
//! one. Letters are written before the numbers that count them; a hub that
//! stops in between finds the letter files and counts from the highest. A
//! member taken off loses its letters only once the directory without it is
//! kept; a hub that stops in between deletes them when it opens, as it does
//! any letters kept for no member.

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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The version the network asks for at least, and whether its managed
    /// machines update by themselves (`fastctx_types::update::Policy`).
    #[serde(default)]
    pub updates: fastctx_types::update::Policy,
    /// The updates machines away are to make once they are back, by the
    /// machine's key: a name it is renamed to keeps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub due: Vec<DueFor>,
}

/// An update kept for the machine with `key` (`fastctx_types::update::Due`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DueFor {
    pub key: PublicKey,
    #[serde(flatten)]
    pub due: fastctx_types::update::Due,
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

/// Where the letters of members taken off wait to be deleted.
pub(crate) const TRASH: &str = "outbox-trash";

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
    /// The account it was registered under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<AccountName>,
    /// What it was told to do then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<fastctx_types::uninstall::Then>,
    /// What it said it could not do of that, the last time it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
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
    /// The member's last session said it reads the records of updates due
    /// and made as another asked.
    #[serde(default)]
    pub due: bool,
    /// Why the hub refused letters of the member's that it may send again,
    /// by number: kept in its count's file (`Taken`), not here.
    #[serde(skip)]
    pub refused: BTreeMap<u64, String>,
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
    /// What this opening cleared that a stop had left: letters kept for no
    /// member, and what could not be cleared.
    pub tidied: Vec<String>,
    /// A move of the hub away from here (`crate::moving`).
    pub outgoing: Option<crate::moving::Outgoing>,
    /// The moves that brought the hub here, oldest first.
    pub moves: Vec<crate::moving::Incoming>,
    /// Moves called off whose new place may still hold the hub's identity,
    /// until it gives it up (`crate::moving::out`).
    pub called_off: Vec<crate::moving::Outgoing>,
    /// Which letters wait for each member, and how far they were sent
    /// (`crate::mailbox`).
    pub(crate) mailboxes: std::collections::HashMap<MachineName, crate::mailbox::Mailbox>,
    /// Told whenever `letters` changed, for its writer
    /// (`crate::mailbox::keep_records`).
    pub(crate) letters_changed: std::sync::Arc<tokio::sync::Notify>,
    /// Counts every change of what the hub keeps of its letters: the
    /// records, and the senders' counts (`crate::records`).
    revision: u64,
    /// How far the records of where the letters stand are written.
    pub(crate) records: std::sync::Arc<crate::records::Records>,
    /// What each sender's count is written through (`Store::write_count`).
    counts: std::collections::HashMap<MachineName, std::sync::Arc<crate::records::Fence>>,
    /// The session of each member whose letters move its count now: one
    /// replaced keeps no more (`crate::control::intake`).
    pub(crate) intake: std::collections::HashMap<MachineName, u64>,
}

/// The most refusals a sender's count keeps, the latest.
const REFUSALS: usize = 256;

/// A sender's count of its own letters, the last of which the hub holds,
/// and why it refused those it refused: kept in a small file of its own,
/// written before the hub says it holds a letter, so a letter sent again
/// after a stop is known, and one refused is told why again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Taken {
    pub epoch: u64,
    pub taken: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refused: BTreeMap<u64, String>,
}

impl Taken {
    /// `entry`'s count as it is now, with its latest refusals.
    pub(crate) fn of(entry: &Letters) -> Self {
        let skip = entry.refused.len().saturating_sub(REFUSALS);
        let refused = entry.refused.iter().skip(skip).map(|(seq, why)| (*seq, why.clone())).collect();
        Self { epoch: entry.epoch, taken: entry.taken, refused }
    }
}

/// A number given to a letter for a member (`Store::next_letter`).
pub(crate) struct Numbered {
    pub seq: u64,
    /// The file it is written to.
    pub path: PathBuf,
    /// The key of the member it is for, when it was numbered.
    pub key: Option<PublicKey>,
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
            // A sender's count is kept in a file of its own, written before
            // the hub said it holds a letter; the records may be older.
            if let Some(kept) = read_taken(dir, name)? {
                if (kept.epoch, kept.taken) != (entry.epoch, entry.taken) {
                    entry.epoch = kept.epoch;
                    entry.taken = kept.taken;
                    changed = true;
                }
                entry.refused = kept.refused;
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
        let mut store = Self {
            dir: dir.to_path_buf(),
            hub,
            book,
            letters,
            worked,
            upgraded,
            tidied: Vec::new(),
            outgoing,
            moves,
            called_off,
            mailboxes: std::collections::HashMap::new(),
            letters_changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            revision: 0,
            records: std::sync::Arc::new(crate::records::Records::new()),
            counts: std::collections::HashMap::new(),
            intake: std::collections::HashMap::new(),
        };
        if changed {
            store.save_letters_now()?;
        }
        // A hub stopped between keeping a rename and moving what it keeps
        // for the machine finishes that now.
        let moved = store.settle_names()?;
        store.upgraded.extend(moved);
        // And one stopped between keeping a removal and setting the letters
        // aside sets them aside now; nothing of it keeps the hub from opening.
        store.tidied = store.forget_strays();
        if let Err(error) = Self::clear_trash(dir) {
            store.tidied.push(format!("emptying the letters set aside: {error}; tried again when the hub opens next"));
        }
        Ok(store)
    }

    /// Sets aside the letters kept for no member of the directory, and
    /// forgets the records of those letters; says what it did.
    fn forget_strays(&mut self) -> Vec<String> {
        let mut said = Vec::new();
        let members: std::collections::BTreeSet<String> = self
            .book
            .directory
            .members
            .iter()
            .flat_map(|member| [member.name.file_name().into_owned(), member.name.key().to_string()])
            .collect();
        let strays: Vec<MachineName> =
            self.letters.keys().filter(|name| self.book.directory.member(name).is_none()).cloned().collect();
        for name in strays {
            self.letters.remove(&name);
            said.push(format!("forgot the letter records of {name}, no member of the hub"));
        }
        if !said.is_empty()
            && let Err(error) = self.save_letters_now()
        {
            said.push(format!("recording that: {error}"));
        }
        if let Ok(counts) = std::fs::read_dir(self.dir.join(TAKEN)) {
            for count in counts.flatten() {
                let name = count.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.')
                    && !members.contains(&name)
                    && let Err(error) = std::fs::remove_file(count.path())
                {
                    said.push(format!("deleting the count of {name}, no member of the hub: {error}"));
                }
            }
        }
        let outbox = self.dir.join("outbox");
        let entries = match std::fs::read_dir(&outbox) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return said,
            Err(error) => {
                said.push(format!("reading {}: {error}", outbox.display()));
                return said;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || members.contains(&name) {
                continue;
            }
            match self.set_aside(&entry.path(), &name) {
                Ok(()) => said.push(format!("set aside the letters kept for {name}, no member of the hub")),
                Err(error) => said.push(format!("setting aside the letters kept for {name}: {error}")),
            }
        }
        said
    }

    /// Moves the letters at `path`, kept for `name`, to the trash.
    fn set_aside(&self, path: &Path, name: &str) -> io::Result<()> {
        let trash = self.dir.join(TRASH);
        ensure_private_dir(&trash)?;
        let unique = u64::from_le_bytes(fastctx_net::sign::random::<8>()?);
        match std::fs::rename(path, trash.join(format!("{name}.{unique:016x}"))) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Deletes the letters set aside. A long removal is not done under any
    /// lock: whoever sets letters aside empties the trash after letting go.
    pub fn clear_trash(dir: &Path) -> io::Result<()> {
        match std::fs::remove_dir_all(dir.join(TRASH)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
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
                let count = taken_path(&self.dir, &old);
                if std::fs::exists(&count)? {
                    std::fs::rename(&count, taken_path(&self.dir, &new))?;
                }
            }
            // What is known of the letters under the old name is read again
            // under the new, once its session wants them; what was writing
            // under the old name writes there no more.
            self.mailboxes.remove(&old);
            self.intake.remove(&old);
            if let Some(fence) = self.counts.remove(&old) {
                fence.close();
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
            self.save_letters_now()?;
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

    /// Has the records of where the letters stand kept: by their writer, off
    /// this lock, soon after (`crate::mailbox::keep_records`).
    pub fn save_letters(&mut self) -> io::Result<()> {
        self.revision += 1;
        self.letters_changed.notify_one();
        Ok(())
    }

    /// Keeps the records of where the letters stand now, before this lock
    /// is let go: as the hub opens, stops, renames a member or hands itself
    /// over, and before deleting letters only the records then number past.
    pub fn save_letters_now(&mut self) -> io::Result<()> {
        self.revision += 1;
        self.records.write(self.revision, &self.dir.join("letters.json"), &json(&self.letters)?)
    }

    /// The records of where the letters stand as they are now, to be
    /// written off this lock: their revision, what they say, and where.
    pub(crate) fn letters_record(&self) -> (u64, BTreeMap<MachineName, Letters>, PathBuf) {
        (self.revision, self.letters.clone(), self.dir.join("letters.json"))
    }

    /// The revision of what is kept of the letters now (`crate::records`).
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Where the letters waiting for `name` are kept.
    pub(crate) fn outbox_of(&self, name: &MachineName) -> io::Result<PathBuf> {
        name.entry_in(&self.dir.join("outbox"))
    }

    /// `name`'s count as it is now, its revision, and the fence it is
    /// written through, to write it off this lock (`write_count`).
    pub(crate) fn count_of(
        &mut self,
        name: &MachineName,
    ) -> Option<(Taken, u64, std::sync::Arc<crate::records::Fence>)> {
        let taken = Taken::of(self.letters.get(name)?);
        self.revision += 1;
        let fence =
            self.counts.entry(name.clone()).or_insert_with(|| std::sync::Arc::new(crate::records::Fence::new()));
        Some((taken, self.revision, fence.clone()))
    }

    /// Writes `name`'s count `taken`, as of `revision`, through `fence`:
    /// before the hub says it holds the letters it counts. One of an
    /// earlier revision that comes late is passed over.
    pub(crate) fn write_count(
        dir: &Path,
        name: &MachineName,
        taken: &Taken,
        fence: &crate::records::Fence,
        revision: u64,
    ) -> io::Result<()> {
        fence
            .write(revision, || {
                ensure_private_dir(&dir.join(TAKEN))?;
                publish(&taken_path(dir, name), &json(taken)?, Permissions::Private)
            })
            .map(|_| ())
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
                due: false,
                refused: BTreeMap::new(),
            };
            self.letters.insert(name.clone(), fresh);
        }
        Ok(self.letters.get_mut(name).expect("just made"))
    }

    /// The number the next letter for `to` takes, and the file it is to be
    /// written to, off this lock (`write_letter`). Until it is written or
    /// given up (`crate::mailbox::kept`, `abandoned`), no letter after it goes
    /// to `to`; one given up leaves a gap, which nothing minds.
    pub(crate) fn next_letter(&mut self, to: &MachineName) -> io::Result<Numbered> {
        let directory = to.entry_in(&self.dir.join("outbox"))?;
        let key = self.book.directory.member(to).map(|member| member.key);
        let entry = self.letters_of(to)?;
        let seq = entry.next;
        entry.next = seq + 1;
        self.save_letters()?;
        crate::mailbox::numbered(self, to, seq);
        Ok(Numbered { seq, path: directory.join(format!("{seq:020}")), key })
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

    /// Sets aside every letter waiting for `name` and forgets its letter
    /// records, once it is taken off the hub and the directory that says so
    /// is kept (`clear_trash` deletes them).
    pub fn forget_letters(&mut self, name: &MachineName) -> io::Result<()> {
        self.letters.remove(name);
        self.mailboxes.remove(name);
        self.intake.remove(name);
        if let Some(fence) = self.counts.remove(name) {
            fence.close();
        }
        self.save_letters()?;
        match std::fs::remove_file(taken_path(&self.dir, name)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let path = name.entry_in(&self.dir.join("outbox"))?;
        self.set_aside(&path, &name.file_name())
    }
}

/// Where the counts of each sender's letters are kept (`Store::write_taken`).
const TAKEN: &str = "taken";

fn taken_path(dir: &Path, name: &MachineName) -> PathBuf {
    dir.join(TAKEN).join(&*name.file_name())
}

/// `name`'s count of its own letters as its file keeps it, if it has one.
fn read_taken(dir: &Path, name: &MachineName) -> io::Result<Option<Taken>> {
    match read(&taken_path(dir, name)) {
        Ok(taken) => Ok(Some(taken)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Writes the letter `message` to `path` (`Store::next_letter`), off any
/// lock: the hub holds it once this returns. Its size.
pub(crate) fn write_letter(path: &Path, message: &Envelope) -> io::Result<u64> {
    if let Some(directory) = path.parent() {
        ensure_private_dir(directory)?;
    }
    let bytes = json(message)?;
    publish(path, &bytes, Permissions::Private)?;
    Ok(bytes.len() as u64)
}

/// The numbers of the letter files in `directory`, and their sizes.
pub(crate) fn letter_numbers(directory: &Path) -> io::Result<Vec<(u64, u64)>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        // Temporary files of an interrupted write start with a dot.
        if let Some(seq) = entry.file_name().to_str().and_then(|name| name.parse::<u64>().ok()) {
            found.push((seq, entry.metadata()?.len()));
        }
    }
    Ok(found)
}

/// The letter numbered `seq` in `directory`; none when it is not there.
pub(crate) fn read_letter(directory: &Path, seq: u64) -> io::Result<Option<Envelope>> {
    match read(&directory.join(format!("{seq:020}"))) {
        Ok(message) => Ok(Some(message)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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
