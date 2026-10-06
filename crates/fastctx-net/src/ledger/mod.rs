//! A town's shared ledger (`world-network.md`, 共享账本): files written once
//! and never changed, such as journal entries and the versions of a town's
//! record, each entry signed by the machine that wrote it.
//!
//! The hub keeps every entry of every town in the order it took them,
//! sealed so only the town's readers open it (`sealed`), and takes an entry
//! only from a member, only when none of the paths it writes is one the
//! town has already, which it tells by their tokens (`naming`): so two
//! machines writing the same version of something cannot both succeed, and
//! nothing written is ever overwritten. Every machine the town spans keeps
//! a copy and checks each entry itself.

pub mod naming;
pub mod sealed;

use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub use fastctx_types::scope::TownId;
pub use naming::{NamingKey, PathToken, path_token, town_id};
pub use sealed::{Item, Reader, SealedEntry, Share, WorldEntry};

use crate::sign::{Bytes, Domain, Signed};
use crate::trust::{Table, WorldId};

/// One entry: the files it adds to a town's ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub world: WorldId,
    pub town: TownName,
    pub author: MachineName,
    pub at: Timestamp,
    /// Set on an entry that writes a version of the town's record: the
    /// machines the town spans from this entry on.
    pub machines: Option<Vec<MachineName>>,
    pub files: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Relative to the town's ledger, names joined by `/`.
    pub path: String,
    pub bytes: Bytes,
}

/// The entry `signed` holds, if its author, a member of `world` now or
/// before, signed it. An author revoked or cut off since is believed for
/// entries it says it wrote before: the hub took them only from members.
pub fn open(signed: &Signed, world: &WorldId, table: &Table) -> Result<Entry, String> {
    let claimed: Entry = signed.claim().map_err(|error| format!("an unreadable ledger entry: {error}"))?;
    if claimed.world != *world {
        return Err(format!("a ledger entry for the World {}", claimed.world));
    }
    let key = table.author_key(&claimed.author, claimed.at)?;
    let entry: Entry = signed.open(&key, Domain::Ledger).map_err(|error| format!("a ledger entry: {error}"))?;
    for file in &entry.files {
        check_path(&file.path)?;
    }
    Ok(entry)
}

/// Whether `path` may name a file in a ledger: relative, `/` between names,
/// and every name one that any system the World runs on can hold as it is.
pub fn check_path(path: &str) -> Result<(), String> {
    let invalid = |why: &str| Err(format!("{path:?} cannot be a path in a ledger: {why}"));
    if path.is_empty() {
        return invalid("it is empty");
    }
    for name in path.split('/') {
        if name.is_empty() || name == "." || name == ".." {
            return invalid("a name in it is empty, `.` or `..`");
        }
        if name.ends_with('.') || name.ends_with(' ') {
            return invalid("a name ends with a dot or a space, which Windows drops");
        }
        if name.chars().any(|c| c.is_control() || "\\:<>\"|?*".contains(c)) {
            return invalid("a name holds a character Windows does not allow");
        }
        let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            return invalid("a name is one Windows reserves for a device");
        }
    }
    Ok(())
}

/// Whether two paths cannot both be in one ledger: the same on a system
/// that ignores case, or one a directory the other is in.
pub fn conflicts(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// Signs an entry as its author.
pub fn sign(key: &crate::sign::SecretKey, entry: &Entry) -> Signed {
    Signed::new(key, Domain::Ledger, entry)
}

/// The size of the answer that hands `signed` to a machine; an entry must
/// fit in one message, so this must not exceed `wire::MAX_CONTROL_FRAME`.
pub fn handed_size(signed: &Signed) -> usize {
    crate::wire::encode(&crate::wire::Down::Told {
        id: u64::MAX,
        answer: Ok(crate::wire::Told::Entries { entries: vec![(u64::MAX, signed.clone())], more: false }),
    })
    .len()
}

/// How far an entry is from fitting in one message once sealed for
/// `readers` members, signed by anyone: a signature, a sealed copy of a key
/// and a path's token are each the same size whoever makes them, and
/// readers are counted with the longest name a machine can have. `Ok` with
/// the bytes to spare, or `Err` with the bytes too many.
pub fn fits(entry: &Entry, readers: usize) -> Result<usize, usize> {
    let key = crate::sign::SecretKey::from_seed(&[0; 32]);
    let inner = sign(&key, entry);
    let longest = MachineName::try_from("m".repeat(32)).expect("a machine name of the longest length");
    let seal = crate::seal::SealKey::from_seed(&[1; 32]).public();
    let readers = vec![Reader { name: longest.clone(), seal }; readers];
    let paths: Vec<String> = entry.files.iter().map(|file| file.path.clone()).collect();
    let naming = NamingKey::from_bytes([0; 32]);
    let town = town_id(&naming, &entry.town);
    let item = sealed::seal_entry(
        &key,
        &entry.world,
        &longest,
        &town,
        Some(&naming),
        &paths,
        entry.machines.clone(),
        &inner,
        &readers,
    )
    .expect("sealing for measuring");
    let sent = crate::wire::encode(&crate::wire::Up::Ask {
        id: u64::MAX,
        question: crate::wire::Question::LedgerWrite { item: item.clone() },
    })
    .len();
    let size = handed_size(&item).max(sent);
    let limit = crate::wire::MAX_CONTROL_FRAME;
    if size <= limit { Ok(limit - size) } else { Err(size - limit) }
}
