//! The account's creation timestamp separates its records from a later account
//! with the same name. Removed incarnations remain archived and are never opened.

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
}

#[derive(Default)]
pub(crate) struct Account {
    pub entries: BTreeMap<(String, String), Entry>,
    pub bytes: usize,
}

pub(crate) struct Slot {
    dir: PathBuf,
    pub records: tokio::sync::Mutex<Account>,
    pub count: AtomicUsize,
    pub bytes: AtomicUsize,
}

impl Slot {
    pub fn path(&self, kind: &str, key: &str) -> PathBuf {
        let digest = Sha256::digest(format!("{kind}\0{key}").as_bytes());
        self.dir.join(format!("{digest:x}.json"))
    }
}

type Identity = (AccountName, Timestamp);

pub(crate) struct Records {
    dir: PathBuf,
    accounts: Mutex<HashMap<Identity, Arc<Slot>>>,
}

impl Records {
    pub fn open(dir: &Path, directory: &fastctx_net::directory::Directory) -> std::io::Result<Self> {
        let records = Self { dir: dir.join("account-records"), accounts: Mutex::default() };
        for owner in &directory.accounts {
            let slot = records.account(&owner.name, owner.opened);
            let files = match std::fs::read_dir(&slot.dir) {
                Ok(files) => files,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut account = slot.records.try_lock().expect("not shared during startup");
            for file in files {
                let path = file?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let bytes = std::fs::read(&path)?;
                let record: Record = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
                let metadata = record.metadata;
                if slot.path(&metadata.kind, &metadata.key) != path {
                    return Err(std::io::Error::other(format!("unexpected record path {}", path.display())));
                }
                account.bytes += bytes.len();
                account
                    .entries
                    .insert((metadata.kind.clone(), metadata.key.clone()), Entry { metadata, size: bytes.len() });
            }
            slot.count.store(account.entries.len(), Ordering::Relaxed);
            slot.bytes.store(account.bytes, Ordering::Relaxed);
        }
        Ok(records)
    }

    pub fn account(&self, name: &AccountName, opened: Timestamp) -> Arc<Slot> {
        self.accounts
            .lock()
            .expect("never poisoned")
            .entry((name.clone(), opened))
            .or_insert_with(|| {
                Arc::new(Slot {
                    dir: self.dir.join(name.as_str()).join(opened.as_nanosecond().to_string()),
                    records: tokio::sync::Mutex::default(),
                    count: AtomicUsize::new(0),
                    bytes: AtomicUsize::new(0),
                })
            })
            .clone()
    }

    /// A deleted account's files stay archived under its former incarnation.
    pub fn settle(&self, directory: &fastctx_net::directory::Directory) {
        self.accounts
            .lock()
            .expect("never poisoned")
            .retain(|(name, opened), _| directory.account(name).is_some_and(|account| account.opened == *opened));
    }

    pub fn health(&self) -> serde_json::Value {
        let accounts = self.accounts.lock().expect("never poisoned");
        serde_json::json!({
            "accounts": accounts.len(),
            "keys": accounts.values().map(|slot| slot.count.load(Ordering::Relaxed)).sum::<usize>(),
            "bytes": accounts.values().map(|slot| slot.bytes.load(Ordering::Relaxed)).sum::<usize>(),
            "body_limit": fastctx_types::account_records::MAX_BODY_BYTES,
            "account_limit": fastctx_types::account_records::MAX_ACCOUNT_BYTES,
            "kind_key_limit": fastctx_types::account_records::MAX_KIND_KEYS,
        })
    }
}

pub(crate) fn read(path: &Path) -> Result<Record, String> {
    let bytes = std::fs::read(path).map_err(fastctx_types::account_records::words::read)?;
    serde_json::from_slice(&bytes).map_err(fastctx_types::account_records::words::read)
}
