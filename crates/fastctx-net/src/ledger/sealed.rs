//! A ledger as the hub keeps it: items it can order and hand out but not
//! read.
//!
//! An entry, signed by its author as before, is sealed whole with a key
//! made for it alone; that key is sealed to each member that may read the
//! town (`crate::trust::Table::readers`), and the copies travel with the
//! entry. When a member becomes a reader later, a member that can open the
//! entry seals its key once more for the newcomer, in an item of its own
//! that only adds a copy: two such items for one member are two identical
//! copies, never a conflict. What stays in the clear is what the hub needs:
//! the town's id, the author, the entry's random id, the machines a
//! version of the town's record says it spans, the tokens of the paths it
//! writes, and who each copy of the key is for.

use chacha20poly1305::aead::{Aead as _, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit as _};
use fastctx_types::MachineName;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::TownId;
use super::naming::{NamingKey, PathToken, path_token};
use crate::seal::{self, SealKey, SealPublic, Sealed};
use crate::sign::{Bytes, Domain, SecretKey, Signature, Signed};
use crate::trust::{Table, WorldId};

/// One item of a town's ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "item")]
pub enum Item {
    Entry(SealedEntry),
    Share(Share),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedEntry {
    pub world: WorldId,
    pub town: TownId,
    pub author: MachineName,
    /// Chosen at random by the author; every sealed copy of the entry's key
    /// is bound to it.
    pub id: Bytes,
    /// Set on an entry that writes a version of the town's record: the
    /// machines the town spans from this entry on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<MachineName>>,
    /// The tokens of the paths it writes (`super::naming`).
    pub paths: Vec<PathToken>,
    /// The entry's key, sealed to each reader.
    pub keys: Vec<Wrapped>,
    /// The entry as its author signed it, sealed with its key.
    pub body: Bytes,
}

/// An entry's key sealed to one member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrapped {
    pub to: MachineName,
    pub key: Bytes,
}

/// The key of the entry numbered `entry`, sealed once more for `to` by a
/// member that could open it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub world: WorldId,
    pub town: TownId,
    pub entry: u64,
    /// The entry's random id, which the copy is bound to.
    pub id: Bytes,
    pub to: MachineName,
    pub by: MachineName,
    pub key: Bytes,
}

impl Item {
    pub fn town(&self) -> &TownId {
        match self {
            Item::Entry(entry) => &entry.town,
            Item::Share(share) => &share.town,
        }
    }

    pub fn world(&self) -> &WorldId {
        match self {
            Item::Entry(entry) => &entry.world,
            Item::Share(share) => &share.world,
        }
    }

    /// Who signed it.
    pub fn signer(&self) -> &MachineName {
        match self {
            Item::Entry(entry) => &entry.author,
            Item::Share(share) => &share.by,
        }
    }
}

/// A member an entry is sealed for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reader {
    pub name: MachineName,
    pub seal: SealPublic,
}

/// The readers of `town` that have sealing keys, by `table`.
pub fn readers(table: &Table, town: &TownId) -> Vec<Reader> {
    table
        .readers(town)
        .filter_map(|member| member.seal.map(|seal| Reader { name: member.name.clone(), seal }))
        .collect()
}

/// An entry's own key.
pub struct EntryKey(Zeroizing<[u8; 32]>);

impl EntryKey {
    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(&(*self.0).into())
    }
}

/// The path token of the World ledger's one entry, the naming key: a second
/// one is refused by the hub as a path written twice.
pub fn naming_token() -> PathToken {
    Bytes(vec![0; 16])
}

fn key_info(world: &WorldId, town: &TownId, id: &Bytes, to: &MachineName) -> Vec<u8> {
    let mut info = b"fastctx-world/3/entry-key\0".to_vec();
    for part in [world.to_string().as_str(), town.as_str(), to.as_str()] {
        info.extend_from_slice(part.as_bytes());
        info.push(0);
    }
    info.extend_from_slice(&id.0);
    info
}

fn body_aad(world: &WorldId, town: &TownId, author: &MachineName, id: &Bytes) -> Vec<u8> {
    let mut aad = b"fastctx-world/3/entry\0".to_vec();
    for part in [world.to_string().as_str(), town.as_str(), author.as_str()] {
        aad.extend_from_slice(part.as_bytes());
        aad.push(0);
    }
    aad.extend_from_slice(&id.0);
    aad
}

/// What an entry carries before sealing: the author's signature and the
/// exact bytes it signed.
fn inner_bytes(inner: &Signed) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64 + inner.data.0.len());
    bytes.extend_from_slice(&inner.sig.to_bytes());
    bytes.extend_from_slice(&inner.data.0);
    bytes
}

fn inner_from(bytes: &[u8]) -> Result<Signed, String> {
    if bytes.len() < 64 {
        return Err("the sealed entry is too short to hold a signature".to_string());
    }
    let (sig, data) = bytes.split_at(64);
    Ok(Signed { data: Bytes(data.to_vec()), sig: Signature::from_bytes(sig.try_into().expect("64 bytes")) })
}

/// Seals `inner`, an entry its author signed, into an item of `town` for
/// `readers`, signed by the author with `key`. `paths` are the paths the
/// entry writes, `machines` what a version of the record says the town spans.
#[allow(clippy::too_many_arguments)]
pub fn seal_entry(
    key: &SecretKey,
    world: &WorldId,
    author: &MachineName,
    town: &TownId,
    naming: Option<&NamingKey>,
    paths: &[String],
    machines: Option<Vec<MachineName>>,
    inner: &Signed,
    readers: &[Reader],
) -> Result<Signed, String> {
    let id = Bytes(crate::sign::random::<16>().map_err(|error| error.to_string())?.to_vec());
    let entry_key = EntryKey(Zeroizing::new(crate::sign::random::<32>().map_err(|error| error.to_string())?));
    let paths = match naming {
        Some(naming) => paths.iter().map(|path| path_token(naming, town, path)).collect(),
        None if *town == TownId::world() => vec![naming_token()],
        None => return Err("this machine does not have the World's naming key yet".to_string()),
    };
    let mut keys = Vec::new();
    for reader in readers {
        let sealed = seal::seal(&reader.seal, &key_info(world, town, &id, &reader.name), &[], &*entry_key.0)?;
        keys.push(Wrapped { to: reader.name.clone(), key: Bytes(sealed.to_bytes()) });
    }
    let body = entry_key
        .cipher()
        .encrypt(&[0_u8; 12].into(), Payload { msg: &inner_bytes(inner), aad: &body_aad(world, town, author, &id) })
        .map_err(|_| "sealing the entry failed".to_string())?;
    let item = Item::Entry(SealedEntry {
        world: world.clone(),
        town: town.clone(),
        author: author.clone(),
        id,
        machines,
        paths,
        keys,
        body: Bytes(body),
    });
    Ok(Signed::new(key, Domain::LedgerItem, &item))
}

/// The item in `signed`, if the member it names as its signer signed it:
/// a member now, or one revoked since, as the hub took it from a member.
pub fn open_item(signed: &Signed, world: &WorldId, table: &Table) -> Result<Item, String> {
    let claimed: Item = signed.claim().map_err(|error| format!("an unreadable ledger item: {error}"))?;
    if claimed.world() != world {
        return Err(format!("a ledger item for the World {}", claimed.world()));
    }
    let signer = claimed.signer();
    let key = match (table.get(signer), table.revoked(signer)) {
        (Some(member), _) => member.key,
        (None, Some(revocation)) => revocation.key,
        (None, None) => return Err(format!("{signer} is not a member")),
    };
    signed.open(&key, Domain::LedgerItem).map_err(|error| format!("a ledger item: {error}"))
}

/// The key of `entry`, from its own copies or from `shares`, as `me` with
/// `seal` opens it. `None` when no copy is for `me`.
pub fn entry_key<'a>(
    entry: &SealedEntry,
    shares: impl IntoIterator<Item = &'a Share>,
    me: &MachineName,
    seal: &SealKey,
) -> Result<Option<EntryKey>, String> {
    let info = key_info(&entry.world, &entry.town, &entry.id, me);
    let mut copies: Vec<&Bytes> =
        entry.keys.iter().filter(|wrapped| wrapped.to == *me).map(|wrapped| &wrapped.key).collect();
    copies.extend(shares.into_iter().filter(|share| share.to == *me && share.id == entry.id).map(|share| &share.key));
    let mut failure = None;
    for copy in copies {
        match Sealed::from_bytes(&copy.0).and_then(|sealed| seal.open(&sealed, &info, &[])) {
            Ok(bytes) if bytes.len() == 32 => {
                return Ok(Some(EntryKey(Zeroizing::new(bytes.try_into().expect("32 bytes")))));
            }
            Ok(_) => failure = Some("a copy of its key is not a key".to_string()),
            Err(error) => failure = Some(error.to_string()),
        }
    }
    match failure {
        Some(reason) => Err(format!("no copy of its key for {me} opens: {reason}")),
        None => Ok(None),
    }
}

/// The entry `entry` holds, as its author signed it.
pub fn open_entry(entry: &SealedEntry, key: &EntryKey) -> Result<Signed, String> {
    let plaintext = key
        .cipher()
        .decrypt(
            &[0_u8; 12].into(),
            Payload { msg: &entry.body.0, aad: &body_aad(&entry.world, &entry.town, &entry.author, &entry.id) },
        )
        .map_err(|_| "the entry does not open with its key: it was changed on the way".to_string())?;
    inner_from(&plaintext)
}

/// Seals the key of the entry numbered `seq` once more, for `to`, as `by`.
pub fn share(
    by_key: &SecretKey,
    by: &MachineName,
    entry: &SealedEntry,
    seq: u64,
    key: &EntryKey,
    to: &Reader,
) -> Result<Signed, String> {
    let sealed = seal::seal(&to.seal, &key_info(&entry.world, &entry.town, &entry.id, &to.name), &[], &*key.0)?;
    let item = Item::Share(Share {
        world: entry.world.clone(),
        town: entry.town.clone(),
        entry: seq,
        id: entry.id.clone(),
        to: to.name.clone(),
        by: by.clone(),
        key: Bytes(sealed.to_bytes()),
    });
    Ok(Signed::new(by_key, Domain::LedgerItem, &item))
}

/// What the World's own ledger holds: its naming key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldEntry {
    pub world: WorldId,
    pub author: MachineName,
    pub at: jiff::Timestamp,
    pub naming: NamingKey,
}
