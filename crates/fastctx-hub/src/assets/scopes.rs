//! The hub's scopes (design §2.1, §6.2, §6.4): each scope's descriptor —
//! its owner library, who reads it, which devices hold which of its stream
//! classes — signed by the hub and kept in its control database as a
//! control fact; and the read gate, which judges a request's accounts
//! against the scope's readers in the directory as it stands at that
//! moment. Readers are a reference into the directory, never a list.

use super::{Assets, Owner, chain};
use fastctx_content::v2::Library;
use fastctx_net::authorization::Action;
use fastctx_net::directory_index::Indexed;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::assets::{Bytes, Descriptor, Fact, Holders, Readers, StreamClass};
use fastctx_types::{AccountName, MachineName};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The scopes, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS asset_scopes (
    key TEXT PRIMARY KEY, owner TEXT NOT NULL, descriptor BLOB NOT NULL, word BLOB NOT NULL
) WITHOUT ROWID;
";

/// A scope as the hub keeps it: its descriptor, as signed and as read.
#[derive(Clone)]
pub(crate) struct Scope {
    pub descriptor: Descriptor,
    pub fact: Fact,
    pub owner: Owner,
}

/// The accounts behind a request from `machine`: those it is logged in as
/// and those it may act for (design §6.4).
pub(crate) fn principal(directory: &Indexed<'_>, machine: &MachineName) -> BTreeSet<AccountName> {
    directory.acts_for(machine)
}

/// Whether `account` reads a scope of `readers` in the directory now.
pub(crate) fn reads(directory: &Indexed<'_>, account: &AccountName, readers: &Readers) -> bool {
    match readers {
        Readers::Conversation { openers, born_on } => {
            fastctx_net::record_ownership::allows(directory, account, openers, born_on.as_ref())
        }
        Readers::Project { town } => directory.authorize(account, Action::Project(town)).is_ok(),
        Readers::Account { account: only } => {
            only == account && directory.account(account).is_some_and(|found| !found.locked)
        }
    }
}

/// Whether a request from `machine` reads the scope.
pub(crate) fn readable(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    principal(directory, machine).iter().any(|account| reads(directory, account, &scope.descriptor.readers))
}

/// Whether `machine` may hold `stream` of the scope (design §2.3): the
/// stream's class names who holds it.
pub(crate) fn holds(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, stream: &str) -> bool {
    let Some(class) = scope.descriptor.class(stream) else { return false };
    match class.holders {
        Holders::Readers => readable(directory, machine, scope),
        Holders::Logged => directory
            .member(machine)
            .is_some_and(|member| member.logins.iter().any(|account| reads(directory, account, &scope.descriptor.readers))),
    }
}

/// The scope `key`, if the hub described it.
pub(crate) fn scope(assets: &Assets, key: &str) -> Result<Option<Scope>, String> {
    let key = key.to_string();
    let found: Option<(String, Vec<u8>, Vec<u8>)> = assets
        .control
        .read_now(move |c| {
            Ok(c.prepare_cached("SELECT owner, descriptor, word FROM asset_scopes WHERE key = ?1")?
                .query_row([&key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?)
        })
        .map_err(|error| error.to_string())?;
    let Some((owner, descriptor, word)) = found else { return Ok(None) };
    let owner = Owner::from_key(&owner).ok_or_else(|| format!("a scope of an unknown owner {owner}"))?;
    let read: Descriptor = serde_json::from_slice(&descriptor).map_err(|error| error.to_string())?;
    Ok(Some(Scope { descriptor: read, fact: Fact { fact: Bytes(descriptor), word: Bytes(word) }, owner }))
}

/// The owner library of `scope`, with the scope installed in it: a scope
/// described just before a stop is installed at its first use.
pub(crate) fn library(assets: &Assets, scope: &Scope) -> Result<Library, String> {
    let library = assets.owners.library(&scope.owner).map_err(|error| error.to_string())?;
    let key = &scope.descriptor.scope;
    if library.holding(key).map_err(|error| error.to_string())?.is_none() {
        library.install(key, scope.fact.fact.0.clone(), scope.descriptor.seq as i64).map_err(|error| error.to_string())?;
    }
    Ok(library)
}

/// Who owns a scope of `readers` (design §6.2): a conversation its first
/// opener, a project its owner, an account itself.
fn owner_of(directory: &Indexed<'_>, readers: &Readers) -> Result<Owner, String> {
    match readers {
        Readers::Conversation { openers, .. } => {
            openers.first().map(|account| Owner::Account(account.to_string())).ok_or("a conversation has an opener".into())
        }
        Readers::Project { town } => directory
            .all
            .projects
            .iter()
            .find(|project| &project.town == town)
            .map(|project| Owner::Account(project.owner.to_string()))
            .ok_or_else(|| format!("there is no project {town}")),
        Readers::Account { account } => Ok(Owner::Account(account.to_string())),
    }
}

/// Describes the scope `key` that a device of `machine` begins (design
/// §2.1): one of the accounts it acts for must be among its readers, and a
/// conversation's openers among those accounts. The descriptor is signed,
/// kept as a control fact and witnessed before it is answered; the same
/// description again is answered with the descriptor kept.
pub(crate) fn describe(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &str,
    readers: Readers,
    classes: Vec<StreamClass>,
) -> Result<Fact, String> {
    if key.is_empty() || key.len() > 128 {
        return Err("a scope's key is 1 to 128 bytes".into());
    }
    if let Some(kept) = scope(assets, key)? {
        if kept.descriptor.readers == readers && kept.descriptor.classes == classes {
            return Ok(kept.fact);
        }
        return Err(format!("the scope {key} is described otherwise"));
    }
    let principal = principal(directory, machine);
    if let Readers::Conversation { openers, .. } = &readers
        && !openers.iter().all(|opener| principal.contains(opener))
    {
        return Err("a conversation's openers are accounts this machine acts for".into());
    }
    if !principal.iter().any(|account| reads(directory, account, &readers)) {
        return Err("none of this machine's accounts reads that scope".into());
    }
    let owner = owner_of(directory, &readers)?;
    let descriptor = Descriptor {
        scope: key.to_string(),
        owner: owner.key(),
        readers,
        classes,
        revision: 1,
        seq: assets.witness.current().0,
    };
    let bytes = serde_json::to_vec(&descriptor).map_err(|error| error.to_string())?;
    let word = assets.signer().sign(&bytes);
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let (key_text, owner_key, stored, signed) = (key.to_string(), owner.key(), bytes.clone(), word.clone());
    let (seq, head) = assets
        .control
        .write_now(256 + bytes.len(), move |c| {
            c.prepare_cached("INSERT INTO asset_scopes (key, owner, descriptor, word) VALUES (?1, ?2, ?3, ?4)")?
                .execute(params![key_text, owner_key, stored, signed])?;
            chain::append(c, "scope", &digest)
        })
        .map_err(|error| error.to_string())?;
    assets.witness.advanced(seq, &head, Vec::new()).map_err(|error| error.to_string())?;
    let kept = Scope { descriptor, fact: Fact { fact: Bytes(bytes), word: Bytes(word) }, owner };
    library(assets, &kept)?;
    Ok(kept.fact)
}
