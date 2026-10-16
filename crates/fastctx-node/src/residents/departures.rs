//! Remote requesters' presence survives the resident cache and the receipt
//! for the letter that changed it. Returning is a durable replacement too.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::Permissions;
use fastctx_types::Address;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[derive(Default, Deserialize, Serialize)]
struct Record {
    departed: bool,
    /// Each hub finishes a letter and its receipt before applying the next,
    /// so only its latest presence letter can still be retried.
    letters: BTreeMap<String, String>,
}

pub(super) struct Departures {
    root: PathBuf,
}

impl Departures {
    /// Preserves the old cache's surviving facts before it is rebuilt.
    pub fn load(world: &WorldDir) -> io::Result<Self> {
        let departures = Self { root: world.departed() };
        fastctx_platform::ensure_private_dir(&departures.root)?;
        let entries = match std::fs::read_dir(world.path().join("resident-cache").join("departed")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(departures),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let target = departures.root.join(path.file_name().expect("directory entry"));
            // A partial migration may be retried, but never replaces a newer
            // durable return with the old cache's departure.
            let record = match read::<Record>(&target)? {
                Some(record) => record,
                None => {
                    let departed = read::<bool>(&path)?.ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("departure record disappeared: {}", path.display()),
                        )
                    })?;
                    Record { departed, ..Record::default() }
                }
            };
            // Reconfirm durability even if the previous migration published
            // this record and then failed, before discarding the old cache.
            publish(&target, &record)?;
        }
        Ok(departures)
    }

    fn path(&self, asker: &Address) -> PathBuf {
        // Keep the old index's key so upgrading can preserve its facts.
        let key = serde_json::to_vec(asker).expect("an address serializes");
        self.root.join(format!("{:x}.json", Sha256::digest(key)))
    }

    pub fn get(&self, asker: &Address) -> io::Result<bool> {
        Ok(read::<Record>(&self.path(asker))?.is_some_and(|record| record.departed))
    }

    /// Publishes the effective presence and the letter that established it
    /// together. An ask has no letter, and keeps the last letters on record.
    /// Replaying an older departure cannot undo a subsequent ask or a return
    /// received through another hub.
    pub fn set(
        &self,
        asker: &Address,
        departed: bool,
        letter: Option<(&str, &crate::letters::Identity)>,
    ) -> io::Result<bool> {
        let path = self.path(asker);
        let mut record = read::<Record>(&path)?.unwrap_or_default();
        let repeated =
            letter.is_some_and(|(_, identity)| record.letters.values().any(|last| identity.recognizes(last)));
        if !repeated {
            record.departed = departed;
        }
        if let Some((hub, identity)) = letter {
            record.letters.insert(hub.to_string(), identity.id.to_string());
        }
        // Publish even on replay: a prior attempt may have replaced the
        // file but failed its durability barrier.
        publish(&path, &record)?;
        Ok(record.departed)
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("reading departure {}: {error}", path.display()))
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn publish(path: &Path, record: &Record) -> io::Result<()> {
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    fastctx_platform::publish(path, &bytes, Permissions::Private)
}
