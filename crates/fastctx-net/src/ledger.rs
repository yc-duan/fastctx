//! A town's shared ledger (`world-network.md`, 共享账本): files written once
//! and never changed, such as journal entries and the versions of a town's
//! record, each entry signed by the machine that wrote it.
//!
//! The hub keeps every entry of every town in the order it took them, and
//! takes an entry only from a member, only when none of its files is a path
//! the town has already: so two machines writing the same version of
//! something cannot both succeed, and nothing written is ever overwritten.
//! Every machine the town spans keeps a copy and checks each entry itself.

use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::sign::{Bytes, Domain, Signed};
use crate::trust::{Table, WorldId};

/// A town as the hub and the wire know it: a name without meaning, so
/// nothing on the way learns what a project is called.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TownId(String);

impl TownId {
    /// The World's own ledger, which every member reads.
    pub fn world() -> Self {
        Self("t-0000000000000000".to_string())
    }

    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(format!("t-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TownId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = text.len() == 18
            && text.starts_with("t-")
            && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a town id")) }
    }
}

impl From<TownId> for String {
    fn from(id: TownId) -> String {
        id.0
    }
}

impl std::fmt::Display for TownId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for TownId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

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
/// before, signed it. An author revoked since is believed for entries it
/// says it wrote before it was revoked: the hub took them only from members.
pub fn open(signed: &Signed, world: &WorldId, table: &Table) -> Result<Entry, String> {
    let claimed: Entry = signed.claim().map_err(|error| format!("an unreadable ledger entry: {error}"))?;
    if claimed.world != *world {
        return Err(format!("a ledger entry for the World {}", claimed.world));
    }
    let key = match (table.get(&claimed.author), table.revoked(&claimed.author)) {
        // A device writes nothing in the ledger: what the user writes from
        // one, a machine's node writes for them.
        (Some(member), _) if !member.kind.is_machine() => {
            return Err(format!("{} is a device, and a device writes nothing in the ledger", claimed.author));
        }
        (Some(member), _) => member.key,
        (None, Some(revocation)) if claimed.at < revocation.at => revocation.key,
        (None, Some(_)) => return Err(format!("{} wrote this after it was revoked", claimed.author)),
        (None, None) => return Err(format!("{} is not a member", claimed.author)),
    };
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

/// How far an entry is from fitting in one message, signed by anyone: a
/// signature is the same size whoever makes it. `Ok` with the bytes to
/// spare, or `Err` with the bytes too many.
pub fn fits(entry: &Entry) -> Result<usize, usize> {
    let signed = sign(&crate::sign::SecretKey::from_seed(&[0; 32]), entry);
    let sent = crate::wire::encode(&crate::wire::Up::Ask {
        id: u64::MAX,
        question: crate::wire::Question::LedgerWrite { entry: signed.clone() },
    })
    .len();
    let size = handed_size(&signed).max(sent);
    let limit = crate::wire::MAX_CONTROL_FRAME;
    if size <= limit { Ok(limit - size) } else { Err(size - limit) }
}
