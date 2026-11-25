//! The account's creation timestamp separates its records from a later account
//! with the same name. Removed incarnations remain archived, opened again only
//! for the conversations described into a body (§29, 2.6).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use fastctx_types::{
    AccountName,
    account_records::{Metadata, Record},
};
use jiff::Timestamp;
use sha2::{Digest, Sha256};

pub(crate) struct Entry {
    pub metadata: Metadata,
    pub size: usize,
    pub history: Option<fastctx_types::history::HistoryRevision>,
    pub conversation: Option<String>,
}

#[derive(Default)]
pub(crate) struct Account {
    pub entries: BTreeMap<(String, String), Entry>,
    pub bytes: usize,
}

impl Account {
    pub fn history(&self, resident: &str) -> impl Iterator<Item = &Entry> {
        let kind = fastctx_types::history::RECORD_KIND;
        let prefix = format!("{resident}/");
        self.entries
            .range((kind.to_owned(), prefix.clone())..)
            .take_while(move |((entry_kind, key), _)| entry_kind == kind && key.starts_with(&prefix))
            .filter(|((_, key), _)| {
                key.split_once('/').is_some_and(|(_, name)| fastctx_types::history::markdown::check_name(name).is_ok())
            })
            .map(|(_, entry)| entry)
    }
}

pub(crate) struct Slot {
    dir: PathBuf,
    pub records: Arc<tokio::sync::Mutex<Account>>,
    pub count: AtomicUsize,
    pub bytes: AtomicUsize,
    /// Its records were read from its disk: as the hub opened, or since, as
    /// the hub let go of a removed account's own (`Records::reopen`).
    loaded: std::sync::atomic::AtomicBool,
}

impl Slot {
    pub fn root(&self) -> &Path {
        &self.dir
    }
    pub fn history(&self, kind: &str, key: &str) -> PathBuf {
        self.path(kind, key).with_extension("versions")
    }
    pub fn path(&self, kind: &str, key: &str) -> PathBuf {
        let digest = Sha256::digest(format!("{kind}\0{key}").as_bytes());
        self.dir.join(format!("{digest:x}.json"))
    }
}

type Identity = (AccountName, Timestamp);

pub(crate) struct Records {
    pub owners: Mutex<crate::owned_records::index::Index>,
    dir: PathBuf,
    accounts: Mutex<HashMap<Identity, Arc<Slot>>>,
    /// Revisions found staged as the records were loaded, settled once the
    /// asset library is open (`settle_staged`).
    staged: Mutex<Vec<(Identity, PathBuf)>>,
}

impl Records {
    pub fn open(dir: &Path, directory: &fastctx_net::directory::Directory) -> std::io::Result<Self> {
        let records = Self {
            dir: dir.join("account-records"),
            accounts: Mutex::default(),
            owners: Mutex::default(),
            staged: Mutex::default(),
        };
        for owner in &directory.accounts {
            records.load(&owner.name, owner.opened)?;
        }
        Ok(records)
    }

    /// Loads the records of `name` as opened at `opened`, during startup: an
    /// account's now, or a removed one's that a body's conversation still
    /// keeps its records in (§29, 2.6).
    pub fn load(&self, name: &AccountName, opened: Timestamp) -> std::io::Result<()> {
        let slot = self.account(name, opened);
        let mut account = slot.records.try_lock().expect("not shared during startup");
        self.fill(name, opened, &slot, &mut account, true)
    }

    /// Reads the records of a removed account, as opened at `opened`, from
    /// its disk where the hub let go of them since it opened, for the hub to
    /// let go of what is purged of it (`crate::purges`): a slot made anew is
    /// otherwise empty, and what it keeps would be taken for none
    /// (codex-d515b194). One read already is as it stands; what was written
    /// to it since it was made stands over what its disk keeps.
    pub fn reopen(
        &self,
        name: &AccountName,
        opened: Timestamp,
        slot: &Slot,
        account: &mut Account,
    ) -> std::io::Result<()> {
        if slot.loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        self.fill(name, opened, slot, account, false)
    }

    /// Reads `slot`'s records from its disk into `account`; as the hub opens,
    /// `staging` the revisions found staged, to be settled. They are read
    /// whole before any of them is taken: one that cannot be read leaves
    /// `account` and the hub's index as they were, to be read again, and not
    /// taken for whole as read in part (codex-87c8bbd9, codex-d515b194). What
    /// `account` holds already, written since its slot was made, stands over
    /// what its disk keeps.
    fn fill(
        &self,
        name: &AccountName,
        opened: Timestamp,
        slot: &Slot,
        account: &mut Account,
        staging: bool,
    ) -> std::io::Result<()> {
        // A skill change committed and not yet written out is written first,
        // into what `account` holds, as each asking does.
        crate::skill_storage::recover(slot, account).map_err(std::io::Error::other)?;
        let files = match fastctx_platform::disk::read_dir(&slot.dir) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                slot.loaded.store(true, Ordering::Release);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let mut read = Account::default();
        let mut owned = Vec::new();
        let mut staged = Vec::new();
        for file in files {
            let path = file?.path();
            if super::staged::is_staged(&path) {
                if staging {
                    staged.push(((name.clone(), opened), path));
                }
                continue;
            }
            if path.file_name().is_some_and(|name| name == "skill-edit.json")
                || path.extension().is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let bytes = fastctx_platform::disk::read(&path)?;
            let record: Record = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
            let metadata = record.metadata.clone();
            if slot.path(&metadata.kind, &metadata.key) != path {
                return Err(std::io::Error::other(format!("unexpected record path {}", path.display())));
            }
            if metadata.kind == fastctx_types::owned_records::KIND && !metadata.deleted {
                let conversation: fastctx_types::owned_records::Conversation =
                    serde_json::from_value(record.body.clone()).map_err(std::io::Error::other)?;
                owned.push((metadata.key.clone(), conversation));
            }
            let history = history_head(&record)?;
            read.bytes += bytes.len();
            read.entries.insert(
                (metadata.kind.clone(), metadata.key.clone()),
                Entry { metadata, size: bytes.len(), history, conversation: record_conversation(&record) },
            );
        }
        for entry in read.entries.values().filter(|entry| {
            entry.metadata.deleted
                && matches!(
                    entry.metadata.kind.as_str(),
                    fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND
                )
        }) {
            super::history::delete(&slot.history(&entry.metadata.kind, &entry.metadata.key))?;
        }
        // Read whole: taken.
        {
            let mut owners = self.owners.lock().expect("never poisoned");
            for (key, conversation) in owned {
                if !account.entries.contains_key(&(fastctx_types::owned_records::KIND.to_string(), key.clone())) {
                    owners.take(name, opened, &key, Some(conversation));
                }
            }
        }
        self.staged.lock().expect("never poisoned").extend(staged);
        for (identity, entry) in std::mem::take(&mut account.entries) {
            if let Some(before) = read.entries.insert(identity, entry) {
                read.bytes -= before.size;
            }
        }
        read.bytes += account.bytes;
        *account = read;
        slot.count.store(account.entries.len(), Ordering::Relaxed);
        slot.bytes.store(account.bytes, Ordering::Relaxed);
        slot.loaded.store(true, Ordering::Release);
        Ok(())
    }

    /// Settles the revisions staged as the hub stopped (`staged::settle`):
    /// `taken` says whether the asset library holds one. One the library
    /// cannot say of stays staged for the next opening. Says what it did.
    pub fn settle_staged(
        &self,
        taken: impl Fn(&AccountName, Timestamp, &Record) -> Result<bool, String>,
    ) -> Result<Vec<String>, String> {
        let staged = std::mem::take(&mut *self.staged.lock().expect("never poisoned"));
        let mut said = Vec::new();
        for ((name, opened), path) in staged {
            let slot = self.account(&name, opened);
            let mut account = slot.records.try_lock().map_err(|_| "the records are shared while the hub opens")?;
            match super::staged::settle(&slot, &mut account, &path, |record| taken(&name, opened, record))? {
                super::staged::Settled::Published => {
                    said.push(format!(
                        "published a revision of {name}'s records the asset library took as the hub stopped"
                    ));
                }
                super::staged::Settled::Dropped => {}
                super::staged::Settled::Unknown(why) => {
                    said.push(format!("left a staged revision of {name}'s records for the next opening: {why}"));
                }
            }
            slot.count.store(account.entries.len(), Ordering::Relaxed);
            slot.bytes.store(account.bytes, Ordering::Relaxed);
        }
        Ok(said)
    }

    pub fn account(&self, name: &AccountName, opened: Timestamp) -> Arc<Slot> {
        self.accounts
            .lock()
            .expect("never poisoned")
            .entry((name.clone(), opened))
            .or_insert_with(|| {
                Arc::new(Slot {
                    dir: self.dir.join(name.as_str()).join(opened.as_nanosecond().to_string()),
                    records: Default::default(),
                    count: AtomicUsize::new(0),
                    bytes: AtomicUsize::new(0),
                    loaded: std::sync::atomic::AtomicBool::new(false),
                })
            })
            .clone()
    }

    /// A deleted account's files stay archived under its former incarnation;
    /// one a body's conversation keeps its records in stays loaded, as the
    /// hub loads it as it starts (§29, 2.6), so its records are written on
    /// where they are.
    pub fn settle(&self, directory: &fastctx_net::directory::Directory) {
        let kept = self.owners.lock().expect("never poisoned").body_openers();
        self.accounts.lock().expect("never poisoned").retain(|(name, opened), slot| {
            directory.account(name).is_some_and(|account| account.opened == *opened)
                || kept.contains(&(name.clone(), *opened))
                // One held still, as by the hub letting go of a removed
                // account's own (`crate::purges`), stays the one slot of its
                // folder: one made anew beside it would be read and written
                // under another lock (codex-d515b194). It goes as a later
                // change settles them, let go of then.
                || Arc::strong_count(slot) > 1
        });
    }

    pub fn health(&self) -> serde_json::Value {
        let accounts = self.accounts.lock().expect("never poisoned");
        serde_json::json!({
            "accounts": accounts.len(),
            "keys": accounts.values().map(|slot| slot.count.load(Ordering::Relaxed)).sum::<usize>(),
            "bytes": accounts.values().map(|slot| slot.bytes.load(Ordering::Relaxed)).sum::<usize>(),
            "body_limit": fastctx_types::account_records::MAX_BODY_BYTES,
            "kind_key_limit": fastctx_types::account_records::MAX_KIND_KEYS,
        })
    }
}

pub(crate) fn read(path: &Path) -> Result<Record, String> {
    let bytes = fastctx_platform::disk::read(path).map_err(fastctx_types::account_records::words::read)?;
    serde_json::from_slice(&bytes).map_err(fastctx_types::account_records::words::read)
}

pub(crate) fn history_head(record: &Record) -> std::io::Result<Option<fastctx_types::history::HistoryRevision>> {
    if record.metadata.kind == fastctx_types::history::RECORD_KIND && !record.metadata.deleted {
        let stored: fastctx_types::history::stored::Stored =
            serde_json::from_value(record.body.clone()).map_err(std::io::Error::other)?;
        stored.validate().map_err(std::io::Error::other)?;
        Ok(Some(stored.revision))
    } else {
        Ok(None)
    }
}

pub(crate) fn record_conversation(record: &Record) -> Option<String> {
    if record.metadata.deleted && record.metadata.kind != "sentinel" {
        return None;
    }
    match record.metadata.kind.as_str() {
        "goal_text" | "goal_history" | "goal_writes" => record.body["resident"].as_str().map(str::to_owned),
        "sentinel" => record.body["definition"]["owner"]["resident"].as_str().map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A folder of its own, gone with it.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("hub-records-{name}-{}", std::process::id()));
            let _ = fastctx_platform::disk::remove_dir_all(&dir);
            fastctx_platform::disk::create_dir_all(&dir).expect("a folder");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fastctx_platform::disk::remove_dir_all(&self.0);
        }
    }

    fn directory() -> fastctx_net::directory::Directory {
        serde_json::from_value(serde_json::json!({"accounts": [], "members": [], "projects": []})).expect("a directory")
    }

    fn record(key: &str, text: &str) -> Record {
        Record {
            metadata: Metadata {
                kind: "note".into(),
                key: key.into(),
                version: 1,
                by: None,
                at: Timestamp::UNIX_EPOCH,
                deleted: false,
            },
            body: serde_json::json!({ "text": text }),
        }
    }

    fn write(slot: &Slot, record: &Record) {
        fastctx_platform::disk::create_dir_all(slot.root()).expect("a folder");
        let path = slot.path(&record.metadata.kind, &record.metadata.key);
        fastctx_platform::disk::write(path, serde_json::to_vec(record).expect("json")).expect("written");
    }

    #[test]
    fn a_slot_read_in_part_is_not_taken_for_whole() {
        let dir = Scratch::new("part");
        let records = Records::open(&dir.0, &directory()).expect("records");
        let (name, opened): (AccountName, _) = ("carol".parse().expect("an account"), Timestamp::UNIX_EPOCH);
        let slot = records.account(&name, opened);
        write(&slot, &record("a", "kept"));
        fastctx_platform::disk::write(slot.path("note", "b"), b"{").expect("written");
        let mut account = Account::default();
        assert!(records.reopen(&name, opened, &slot, &mut account).is_err());
        assert!(account.entries.is_empty(), "nothing of a slot read in part is taken");
        assert!(!slot.loaded.load(Ordering::Acquire), "a slot read in part is read again");
        write(&slot, &record("b", "mended"));
        // Written to since the slot was made: it stands over the disk.
        let mut written = record("a", "newer");
        written.metadata.version = 2;
        account.entries.insert(
            ("note".into(), "a".into()),
            Entry { metadata: written.metadata.clone(), size: 7, history: None, conversation: None },
        );
        account.bytes = 7;
        records.reopen(&name, opened, &slot, &mut account).expect("read whole");
        assert_eq!(account.entries.len(), 2);
        assert_eq!(account.entries[&("note".to_string(), "a".to_string())].metadata.version, 2);
        let on_disk = fastctx_platform::disk::metadata(slot.path("note", "b")).expect("a file").len() as usize;
        assert_eq!(account.bytes, 7 + on_disk);
        assert!(slot.loaded.load(Ordering::Acquire));
    }

    #[test]
    fn a_slot_held_stays_the_one_of_its_folder() {
        let dir = Scratch::new("held");
        let records = Records::open(&dir.0, &directory()).expect("records");
        let (name, opened): (AccountName, _) = ("carol".parse().expect("an account"), Timestamp::UNIX_EPOCH);
        let held = records.account(&name, opened);
        records.settle(&directory());
        assert!(Arc::ptr_eq(&held, &records.account(&name, opened)), "a slot held is not let go of");
        drop(held);
        records.settle(&directory());
        assert!(records.accounts.lock().expect("never poisoned").is_empty(), "let go of once no one holds it");
    }
}
