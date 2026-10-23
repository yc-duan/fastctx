//! A project's shared ledger (`world-network.md`, 共享账本): files written
//! once and never changed, such as journal entries and the versions of a
//! project's record, each entry signed by the machine or device that wrote
//! it.
//!
//! The hub keeps every entry of every project in the order it took them.
//! It takes an entry only from the member that signed it, and only when
//! none of the paths it writes is one the project has already: so two
//! machines writing the same version of something cannot both succeed, and
//! nothing written is ever overwritten. Every machine the project spans
//! keeps a copy and checks each entry's signature itself, which is how a
//! copy changed on some machine's disk is told.

use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::HubId;
use crate::sign::{Bytes, Domain, PublicKey, SecretKey, Signed};

/// One entry: the files it adds to a project's ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub hub: HubId,
    pub town: TownId,
    pub author: MachineName,
    /// The author's key, which signed the entry. The hub took the entry
    /// only from the member holding it.
    pub key: PublicKey,
    pub at: Timestamp,
    /// Set on an entry that writes a version of the project's record: the
    /// machines the project spans from this entry on.
    pub machines: Option<Vec<MachineName>>,
    pub files: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Relative to the project's ledger, names joined by `/`.
    pub path: String,
    pub bytes: Bytes,
}

/// The entry `signed` holds, if the key it names signed it, for `town` of
/// `hub`, with paths a ledger can hold.
pub fn open(signed: &Signed, hub: &HubId, town: &TownId) -> Result<Entry, String> {
    let claimed: Entry = signed.claim().map_err(|error| format!("an unreadable ledger entry: {error}"))?;
    if claimed.hub != *hub || claimed.town != *town {
        return Err(format!("a ledger entry for the project {} of the hub {}", claimed.town, claimed.hub));
    }
    let entry: Entry = signed.open(&claimed.key, Domain::Ledger).map_err(|error| format!("a ledger entry: {error}"))?;
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
pub fn sign(key: &SecretKey, entry: &Entry) -> Signed {
    Signed::new(key, Domain::Ledger, entry)
}

/// The encoded size of the answer that hands `signed` to a machine.
pub fn handed_size(signed: &Signed) -> usize {
    crate::wire::encode(&crate::wire::Down::Told {
        id: u64::MAX,
        answer: Ok(crate::wire::Told::Entries { entries: vec![(u64::MAX, signed.clone())], more: false }),
    })
    .len()
}

/// Space left in the multipart envelope, including its signature and encoding.
pub fn fits(entry: &Entry) -> Result<usize, usize> {
    let signed = sign(&SecretKey::from_seed(&[0; 32]), entry);
    let sent = crate::wire::encode(&crate::wire::Up::Ask {
        id: u64::MAX,
        question: crate::wire::Question::LedgerWrite { town: entry.town.clone(), entry: signed.clone() },
    })
    .len();
    let size = handed_size(&signed).max(sent);
    let limit = crate::wire::PARTS_AT_MOST;
    if size <= limit { Ok(limit - size) } else { Err(size - limit) }
}
