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
use std::collections::{BTreeMap, BTreeSet};

/// The scopes, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS asset_scopes (
    key TEXT PRIMARY KEY, owner TEXT NOT NULL, descriptor BLOB NOT NULL, word BLOB NOT NULL,
    -- The chain's number when it was described: the order machines list
    -- what they may hold in (§6.6).
    seq INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS asset_scopes_by_owner ON asset_scopes (owner);
";

/// Brings a control database from before scopes were numbered up to them
/// (BB09): the column, then its index.
pub(crate) fn migrate(c: &fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()> {
    let numbered = c.prepare("SELECT 1 FROM pragma_table_info('asset_scopes') WHERE name = 'seq'")?.exists([])?;
    if !numbered {
        c.execute_batch("ALTER TABLE asset_scopes ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;")?;
    }
    c.execute_batch("CREATE INDEX IF NOT EXISTS asset_scopes_by_seq ON asset_scopes (seq, key);")?;
    Ok(())
}

/// A scope as the hub keeps it: its descriptor, as signed and as read.
#[derive(Clone)]
pub(crate) struct Scope {
    pub descriptor: Descriptor,
    pub fact: Fact,
    pub owner: Owner,
    /// Who the bodies' members are, for a scope a body owns.
    pub bodies: Option<std::sync::Arc<super::bodies::Bodies>>,
}

/// The accounts behind a request from `machine`: those it is logged in as
/// and those it may act for (design §6.4).
pub(crate) fn principal(directory: &Indexed<'_>, machine: &MachineName) -> BTreeSet<AccountName> {
    directory.acts_for(machine)
}

/// Who the account, machine or project `key` names is now
/// (`account:NAME`, `machine:NAME`, `project:TOWN`), as a descriptor's
/// `born` keeps it.
fn identity(directory: &Indexed<'_>, key: &str) -> Option<String> {
    let opened = |name: &str| {
        let name = AccountName::try_from(name.to_string()).ok()?;
        directory.account(&name).map(|account| account.opened.as_millisecond().to_string())
    };
    match key.split_once(':')? {
        ("account", name) => opened(name),
        // A machine is its key and its owner as opened: neither a newcomer
        // taking its name nor an owner's account made again inherits it.
        ("machine", name) => {
            let name = MachineName::try_from(name.to_string()).ok()?;
            let member = directory.member(&name)?;
            let home = member.home.as_ref().map(|home| format!("{home}@{}", opened(home.as_str()).unwrap_or_default()));
            Some(format!("{}:{}", member.key, home.unwrap_or_default()))
        }
        // A project is its town, whoever owns it now: a project handed over
        // stays itself.
        ("project", town) => directory.all.projects.iter().any(|project| project.town.to_string() == town).then(|| town.to_string()),
        _ => None,
    }
}

/// Who each account, machine and project `readers` names is now: kept in
/// the descriptor the hub signs.
fn identities(directory: &Indexed<'_>, readers: &Readers) -> Result<BTreeMap<String, String>, String> {
    let keys: Vec<String> = match readers {
        Readers::Conversation { openers, born_on } => openers
            .iter()
            .map(|opener| format!("account:{opener}"))
            .chain(born_on.iter().map(|machine| format!("machine:{machine}")))
            .collect(),
        Readers::Project { town } => vec![format!("project:{town}")],
        Readers::Account { account } => vec![format!("account:{account}")],
        Readers::Body { opener, .. } => vec![format!("account:{opener}")],
    };
    keys.into_iter()
        .map(|key| identity(directory, &key).map(|found| (key.clone(), found)).ok_or_else(|| format!("there is no {key}")))
        .collect()
}

/// Whether `key` is now who it was when the scope was described.
fn same(directory: &Indexed<'_>, born: &BTreeMap<String, String>, key: &str) -> bool {
    born.get(key).is_some_and(|was| identity(directory, key).as_ref() == Some(was))
}

/// Whether `account` reads a scope of `readers`, described when `born` held,
/// in the directory now: an account, machine or project made again under a
/// name the readers name stands for nothing of it.
pub(crate) fn reads(
    directory: &Indexed<'_>,
    account: &AccountName,
    readers: &Readers,
    born: &BTreeMap<String, String>,
    bodies: Option<&super::bodies::Bodies>,
) -> bool {
    reads_known(directory, account, readers, born, bodies).unwrap_or(false)
}

/// As `reads`, an error where it cannot be told now: a body's members that
/// could not be read are not taken for none (AE04).
pub(crate) fn reads_known(
    directory: &Indexed<'_>,
    account: &AccountName,
    readers: &Readers,
    born: &BTreeMap<String, String>,
    bodies: Option<&super::bodies::Bodies>,
) -> std::io::Result<bool> {
    Ok(match readers {
        // The body's members as they are, or as they were frozen, and its
        // opener as it was.
        Readers::Body { body, opener } => {
            let Some(found) = directory.account(account).filter(|found| !found.locked) else { return Ok(false) };
            if account == opener && same(directory, born, &format!("account:{opener}")) {
                return Ok(true);
            }
            match bodies {
                Some(bodies) => bodies.members(directory.all, body)?.contains(&format!("{account}@{}", found.opened.as_millisecond())),
                None => false,
            }
        }
        Readers::Conversation { openers, born_on } => {
            let openers: Vec<AccountName> =
                openers.iter().filter(|opener| same(directory, born, &format!("account:{opener}"))).cloned().collect();
            let born_on = born_on.as_ref().filter(|machine| same(directory, born, &format!("machine:{machine}")));
            fastctx_net::record_ownership::allows(directory, account, &openers, born_on)
        }
        Readers::Project { town } => {
            same(directory, born, &format!("project:{town}")) && directory.authorize(account, Action::Project(town)).is_ok()
        }
        Readers::Account { account: only } => {
            only == account
                && same(directory, born, &format!("account:{only}"))
                && directory.account(account).is_some_and(|found| !found.locked)
        }
    })
}

/// Whether anyone at all may still read the scope: an account its readers
/// name is still who it was, the machine it was born on still is, its
/// project is still there, or its body still has members (design §6.2). An
/// error where that cannot be known: never taken for no one.
pub(crate) fn anyone_reads(directory: &Indexed<'_>, scope: &Scope) -> std::io::Result<bool> {
    let descriptor = &scope.descriptor;
    let born = &descriptor.born;
    Ok(match &descriptor.readers {
        Readers::Account { account } => same(directory, born, &format!("account:{account}")),
        Readers::Conversation { openers, born_on } => {
            openers.iter().any(|opener| same(directory, born, &format!("account:{opener}")))
                || born_on.as_ref().is_some_and(|machine| same(directory, born, &format!("machine:{machine}")))
        }
        Readers::Project { town } => same(directory, born, &format!("project:{town}")),
        Readers::Body { body, opener } => {
            if same(directory, born, &format!("account:{opener}")) {
                return Ok(true);
            }
            let bodies = scope.bodies.as_ref().ok_or_else(|| std::io::Error::other("a body's scope without its bodies"))?;
            !bodies.members(directory.all, body)?.is_empty()
        }
    })
}

/// Whether a request from `machine` reads the scope.
pub(crate) fn readable(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    readable_known(directory, machine, scope).unwrap_or(false)
}

/// As `readable`, an error where it cannot be told now.
fn readable_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    let descriptor = &scope.descriptor;
    for account in principal(directory, machine) {
        if reads_known(directory, &account, &descriptor.readers, &descriptor.born, scope.bodies.as_deref())? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether every account a machine is shared with whole, a share still
/// waiting for the machine to clear among them, reads the scope: a shared
/// machine holds only what all its holders may see (design §6.3,
/// `sync-replica-holders`).
/// Judged as the directory will be once the shares waiting are in force:
/// what a holder reads through holding the machine counts already.
pub(crate) fn sharers_read(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    sharers_read_known(directory, machine, scope).unwrap_or(false)
}

/// As `sharers_read`, an error where it cannot be told now.
fn sharers_read_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    let descriptor = &scope.descriptor;
    let bodies = scope.bodies.as_deref();
    let machine = directory.canonical(machine).clone();
    let shares: Vec<usize> = directory.shares_by_machine(&machine).to_vec();
    let all_read = |directory: &Indexed<'_>| -> std::io::Result<bool> {
        for &at in &shares {
            if !reads_known(directory, &directory.all.shares[at].account, &descriptor.readers, &descriptor.born, bodies)? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    if shares.iter().all(|&at| directory.all.shares[at].active()) {
        return all_read(directory);
    }
    let mut would = directory.all.clone();
    for share in would.shares.iter_mut().filter(|share| share.machine == machine) {
        std::sync::Arc::make_mut(share).pending = None;
    }
    let index = fastctx_net::directory_index::Index::new(&would);
    all_read(&Indexed::new(&would, &index))
}

/// Whether `machine` may hold any stream of the scope (design §2.3, §6.6),
/// an error where who reads it cannot be told now (AE04): every reading of a body's members it makes, not the first only,
/// is never taken for no one (AF02).
pub(crate) fn may_hold_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    if !readable_known(directory, machine, scope)? || !sharers_read_known(directory, machine, scope)? {
        return Ok(false);
    }
    let descriptor = &scope.descriptor;
    for class in &descriptor.classes {
        match class.holders {
            Holders::Readers => return Ok(true),
            Holders::Logged => {
                if let Some(member) = directory.member(machine) {
                    for account in &member.logins {
                        if reads_known(directory, account, &descriptor.readers, &descriptor.born, scope.bodies.as_deref())? {
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }
    Ok(false)
}


/// Whether `machine` may hold `stream` of the scope (design §2.3): the
/// stream's class names who holds it, and a machine shared whole holds only
/// what each of its holders reads.
pub(crate) fn holds(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, stream: &str) -> bool {
    let descriptor = &scope.descriptor;
    let Some(class) = descriptor.class(stream) else { return false };
    if !sharers_read(directory, machine, scope) {
        return false;
    }
    match class.holders {
        Holders::Readers => readable(directory, machine, scope),
        Holders::Logged => directory.member(machine).is_some_and(|member| {
            member.logins.iter().any(|account| reads(directory, account, &descriptor.readers, &descriptor.born, scope.bodies.as_deref()))
        }),
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
    let Some((owner, descriptor, mut word)) = found else { return Ok(None) };
    let owner = Owner::from_key(&owner).ok_or_else(|| format!("a scope of an unknown owner {owner}"))?;
    let read: Descriptor = serde_json::from_slice(&descriptor).map_err(|error| error.to_string())?;
    // A stop came between its chain head and its word; or the word is under
    // a key the hub turned from since, which a device pinned to the key in
    // use now does not take (BB10): said again under that key.
    if word.is_empty() || !assets.keys().heard_now(&descriptor, &word) {
        assets.catch_up().map_err(|error| error.to_string())?;
        word = assets.signer().sign(&descriptor);
        let (key, signed) = (read.scope.clone(), word.clone());
        assets
            .control
            .write_now(256 + signed.len(), move |c| {
                c.prepare_cached("UPDATE asset_scopes SET word = ?2 WHERE key = ?1")?.execute(params![key, signed])?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
    }
    let bodies = matches!(read.readers, Readers::Body { .. }).then(|| assets.bodies.clone());
    Ok(Some(Scope { descriptor: read, fact: Fact { fact: Bytes(descriptor), word: Bytes(word) }, owner, bodies }))
}

/// The owner library of `scope`, with the scope installed in it: a scope
/// described just before a stop is installed at its first use.
pub(crate) fn library(assets: &Assets, scope: &Scope) -> Result<Library, String> {
    let library = assets.library(&scope.owner).map_err(|error| error.to_string())?;
    let key = &scope.descriptor.scope;
    if library.holding(key).map_err(|error| error.to_string())?.is_none() {
        library.install(key, scope.fact.fact.0.clone(), scope.descriptor.seq as i64).map_err(|error| error.to_string())?;
    }
    Ok(library)
}

/// The machine shared whole a project was opened on, if it was: the project
/// is that machine's body's, read by the project's own readers.
fn project_body(directory: &Indexed<'_>, town: &fastctx_types::access::TownId) -> Option<(String, MachineName)> {
    let project = directory.all.projects.iter().find(|project| &project.town == town)?;
    let machine = directory.canonical(project.shared_on.as_ref()?).clone();
    Some((super::bodies::body_of(directory.all, &machine)?, machine))
}

/// Who owns a scope of `readers` (design §6.2): a conversation its first
/// opener, a project its owner — or, opened on a machine shared whole, that
/// machine's body — an account itself; an account as it is now, since it
/// was opened.
fn owner_of(directory: &Indexed<'_>, readers: &Readers) -> Result<Owner, String> {
    let account = match readers {
        // What a body owns is in the body's own library, no member's.
        Readers::Body { body, .. } => return Ok(Owner::Body(body.clone())),
        Readers::Conversation { openers, .. } => openers.first().cloned().ok_or("a conversation has an opener")?,
        Readers::Project { town } => {
            if let Some((body, _)) = project_body(directory, town) {
                return Ok(Owner::Body(body));
            }
            directory
                .all
                .projects
                .iter()
                .find(|project| &project.town == town)
                .map(|project| project.owner.clone())
                .ok_or_else(|| format!("there is no project {town}"))?
        }
        Readers::Account { account } => account.clone(),
    };
    let opened = directory.account(&account).ok_or_else(|| format!("there is no account {account}"))?.opened;
    Ok(Owner::Account(account.to_string(), opened.as_millisecond()))
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
    if key.len() > 128 {
        return Err("a scope's key is at most 128 bytes".into());
    }
    let prefix = readers.key_prefix(assets.id.as_str());
    if !key.starts_with(&prefix) || key.len() == prefix.len() {
        return Err(format!("a scope of those readers has its key under {prefix}"));
    }
    if let Readers::Conversation { born_on: Some(born_on), .. } = &readers
        && directory.canonical(born_on) != directory.canonical(machine)
    {
        return Err("a conversation born on a machine is described by that machine".into());
    }
    if let Some(kept) = scope(assets, key)? {
        // Read through the gate like any other answer.
        if !readable(directory, machine, &kept) {
            return Err(format!("none of this machine's accounts reads the scope {key}"));
        }
        if kept.descriptor.readers == readers && kept.descriptor.classes == classes {
            // Answered again only once its chain head is outside the
            // directory, as the first answer was to be.
            assets.catch_up().map_err(|error| error.to_string())?;
            return Ok(kept.fact);
        }
        return Err(format!("the scope {key} is described otherwise"));
    }
    let principal = principal(directory, machine);
    let born = identities(directory, &readers)?;
    if let Readers::Conversation { openers, .. } = &readers
        && !openers.iter().all(|opener| principal.contains(opener))
    {
        return Err("a conversation's openers are accounts this machine acts for".into());
    }
    let bodies = matches!(readers, Readers::Body { .. }).then(|| assets.bodies.clone());
    if let Readers::Body { body, opener } = &readers {
        // A body's scope is described by its machine, as that body, and
        // opened by an account it acts for.
        if super::bodies::body_of(directory.all, machine).as_deref() != Some(body.as_str()) {
            return Err("a body's scope is described by the machine that is that body".into());
        }
        if !principal.contains(opener) {
            return Err("a body's opener is an account this machine acts for".into());
        }
        assets.bodies.known(directory.all, body, directory.canonical(machine)).map_err(|error| error.to_string())?;
    }
    if !principal.iter().any(|account| reads(directory, account, &readers, &born, bodies.as_deref())) {
        return Err("none of this machine's accounts reads that scope".into());
    }
    let owner = owner_of(directory, &readers)?;
    // A project kept in a body's library: the body's members are kept from
    // now, as for the body's own scopes.
    if let Readers::Project { town } = &readers
        && let Some((body, machine)) = project_body(directory, town)
    {
        assets.bodies.known(directory.all, &body, &machine).map_err(|error| error.to_string())?;
    }
    let descriptor = Descriptor {
        scope: key.to_string(),
        hub: assets.id.clone(),
        owner: owner.key(),
        readers,
        born,
        classes,
        revision: 1,
        seq: assets.witness.current().0,
    };
    let bytes = serde_json::to_vec(&descriptor).map_err(|error| error.to_string())?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let (key_text, owner_key, stored) = (key.to_string(), owner.key(), bytes.clone());
    let (seq, head) = assets
        .control
        .write_now(256 + bytes.len(), move |c| {
            // Never into a library being deleted (design §6.2).
            let added = c
                .prepare_cached(
                    "INSERT INTO asset_scopes (key, owner, descriptor, word, seq)
                     SELECT ?1, ?2, ?3, x'', 0 WHERE NOT EXISTS (SELECT 1 FROM asset_deleting WHERE owner = ?2)",
                )?
                .execute(params![key_text, owner_key, stored])?;
            if added == 0 {
                return Ok(None);
            }
            let (seq, head) = chain::append(c, "scope", &digest)?;
            // Listed in the order the scopes were committed in, by the
            // number this very commit took: one committed late is never
            // behind a listing that went past it (AD03).
            c.prepare_cached("UPDATE asset_scopes SET seq = ?2 WHERE key = ?1")?.execute(params![key_text, seq as i64])?;
            Ok(Some((seq, head)))
        })
        .map_err(|error| error.to_string())?
        .ok_or("the library the scope belongs in is being deleted; describe it again later")?;
    assets.witness.advanced(seq, &head).map_err(|error| error.to_string())?;
    // Signed now, bound to the head the descriptor went into: a device that
    // saw it carries a head a hub put back to before it does not hold.
    let word = assets.signer().sign(&bytes);
    let (key_text, signed) = (key.to_string(), word.clone());
    assets
        .control
        .write_now(256 + word.len(), move |c| {
            c.prepare_cached("UPDATE asset_scopes SET word = ?2 WHERE key = ?1")?.execute(params![key_text, signed])?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    let kept = Scope { descriptor, fact: Fact { fact: Bytes(bytes), word: Bytes(word) }, owner, bodies };
    library(assets, &kept)?;
    Ok(kept.fact)
}
