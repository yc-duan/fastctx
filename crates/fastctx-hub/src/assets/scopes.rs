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
    seq INTEGER NOT NULL DEFAULT 0,
    -- The machine that described a conversation born assisting an account,
    -- as it was (its key and its owner as opened): the one machine that
    -- makes it, holding it whoever else holds that machine (§28.47).
    source TEXT
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
    // And from before the machine making a scope was kept (§28.47).
    let sourced = c.prepare("SELECT 1 FROM pragma_table_info('asset_scopes') WHERE name = 'source'")?.exists([])?;
    if !sourced {
        c.execute_batch("ALTER TABLE asset_scopes ADD COLUMN source TEXT;")?;
    }
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
    /// The machine that makes it, for a conversation born assisting an
    /// account: as it was when it described it (`machine_now`).
    pub source: Option<String>,
}

/// A scope's row in the control database: its owner, its descriptor and the
/// hub's word on it as kept, and the machine that makes it.
type Row = (String, Vec<u8>, Vec<u8>, Option<String>);

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
        Readers::Hub => Vec::new(),
        Readers::Working { account, machine } => vec![format!("account:{account}"), format!("machine:{machine}")],
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
        // The hub's super administrators, as they are now.
        Readers::Hub => directory.all.is_super(account),
        // The account while it can still work on the machine: the gate
        // closes the moment it cannot.
        Readers::Working { account: only, machine } => {
            only == account
                && same(directory, born, &format!("account:{only}"))
                && same(directory, born, &format!("machine:{machine}"))
                && directory.account(account).is_some_and(|found| !found.locked)
                && principal(directory, machine).contains(account)
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
        Readers::Hub => directory.all.accounts.iter().any(|account| account.is_super()),
        // Its account and machine still themselves: one that cannot work
        // there for now may again.
        Readers::Working { account, machine } => {
            same(directory, born, &format!("account:{account}")) && same(directory, born, &format!("machine:{machine}"))
        }
    })
}

/// Whether a request from `machine` reads the scope: whether it holds any of
/// it, however (`hold`).
pub(crate) fn readable(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    hold(directory, machine, scope) != Hold::No
}

/// Whether `key` names a body's scope of its machine's own: its log, and what
/// it did and spent (§29, 2.5).
pub(crate) fn machines_own(key: &str) -> bool {
    key.ends_with("/log") || key.ends_with("/records")
}

/// Whether `scope` is a scope of the body `machine` is now of its own: one
/// it writes and holds whoever is logged in there — none, on a machine its
/// owner manages (PP10, N08).
fn own_log(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    matches!(&scope.descriptor.readers, Readers::Body { body, .. }
        if machines_own(&scope.descriptor.scope)
            && super::bodies::body_of(directory.all, machine).as_deref() == Some(body.as_str()))
}

/// How a machine holds a scope, if at all (design §6.3, §6.6, §28.47): as a
/// copy only what every account that holds the machine reads; what it makes
/// itself though they do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    No,
    /// What it did and spent for an account it works for: the streams of the
    /// account's records named for it, no other machine's (L12).
    Named,
    /// What it makes itself: a conversation it runs for an account it
    /// assists, what an account does on it. Its readers' class, never what
    /// only those logged in hold.
    Made,
    /// A copy: read by an account logged in on it, and, on a machine shared
    /// whole, by its owner and every account it is shared with.
    Copy,
    /// Its own body's log and records, whole.
    Own,
}

/// How `machine` holds the scope, none where that cannot be told now.
pub(crate) fn hold(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> Hold {
    hold_known(directory, machine, scope).unwrap_or(Hold::No)
}

/// As `hold`, an error where who reads the scope cannot be told now (AE04):
/// every reading of a body's members it makes, not the first only, is never
/// taken for no one (AF02).
pub(crate) fn hold_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<Hold> {
    if own_log(directory, machine, scope) {
        return Ok(Hold::Own);
    }
    // A project's ledger, described before, is read by none until it is
    // kept apart by machine (O10).
    if matches!(&scope.descriptor.readers, Readers::Project { .. }) {
        return Ok(Hold::No);
    }
    if logins_read_known(directory, machine, scope)? && holders_read_known(directory, machine, scope)? {
        return Ok(Hold::Copy);
    }
    if made_here(directory, machine, scope) {
        return Ok(Hold::Made);
    }
    if works_for(directory, machine, scope) {
        return Ok(Hold::Named);
    }
    Ok(Hold::No)
}

/// Whether an account logged in on `machine` reads the scope: a copy comes
/// with a login there, never with an account the machine only works for
/// (`placement-one-rule` 2026-10-21, 2026-11-10).
fn logins_read_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    let descriptor = &scope.descriptor;
    let Some(member) = directory.member(machine) else { return Ok(false) };
    for account in &member.logins {
        if directory.may_act_for(account, &member.name)
            && reads_known(directory, account, &descriptor.readers, &descriptor.born, scope.bodies.as_deref())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether every account that holds `machine` reads the scope, where it is
/// shared whole: its owner and each account it is shared with, a share still
/// waiting for the machine to clear among them. A machine shared whole holds
/// as a copy only what all its holders may see, its owner's own no less than
/// another's (design §6.3, `placement-one-rule` 2026-11-10,
/// `sync-replica-holders`). Judged as the directory will be once the shares
/// waiting are in force: what a holder reads through holding the machine
/// counts already.
fn holders_read_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    let descriptor = &scope.descriptor;
    let bodies = scope.bodies.as_deref();
    let machine = directory.canonical(machine).clone();
    let shares: Vec<usize> = directory.shares_by_machine(&machine).to_vec();
    if shares.is_empty() {
        return Ok(true);
    }
    let owner = directory.member(&machine).and_then(|member| member.home.clone());
    let all_read = |directory: &Indexed<'_>| -> std::io::Result<bool> {
        let sharers = shares.iter().map(|&at| &directory.all.shares[at].account);
        for account in owner.iter().chain(sharers) {
            if !reads_known(directory, account, &descriptor.readers, &descriptor.born, bodies)? {
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

/// The machine `machine` is now, as a scope it describes keeps it: its key
/// and its owner as opened (`identity`).
pub(super) fn machine_now(directory: &Indexed<'_>, machine: &MachineName) -> Option<String> {
    identity(directory, &format!("machine:{}", directory.canonical(machine)))
}

/// Whether `machine` makes the scope itself (§28.47): a conversation born
/// assisting an account, which this very machine described as it ran it,
/// while it still acts for an account that opened it; or what an account
/// does on it. Never another conversation of the account it assists
/// (`placement-one-rule` 2026-10-21, 2026-11-10).
fn made_here(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    let descriptor = &scope.descriptor;
    let read = |account: &AccountName| reads(directory, account, &descriptor.readers, &descriptor.born, None);
    match &descriptor.readers {
        Readers::Working { account, machine: on } => directory.same(on, machine) && read(account),
        Readers::Conversation { openers, born_on: None } => {
            scope.source.as_ref().is_some_and(|source| machine_now(directory, machine).as_ref() == Some(source))
                && principal(directory, machine).iter().any(|account| openers.contains(account) && read(account))
        }
        _ => false,
    }
}

/// Whether `machine` works for the account whose records the scope is, an
/// account that does not hold it: what it did and spent for it, it keeps
/// there in streams named for it (L12). What it does for those that hold it
/// goes into its body's records, shared whole, or is theirs to read as a
/// copy (§29 2.5).
fn works_for(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    let descriptor = &scope.descriptor;
    let Readers::Account { account } = &descriptor.readers else { return false };
    principal(directory, machine).contains(account)
        && !directory.holds(account, machine)
        && reads(directory, account, &descriptor.readers, &descriptor.born, None)
}

/// Whether `stream` of an account's records is one `machine` keeps what it
/// did, ran or spent in: `activity/<key>`, `usage/<key>`, `jobs/<key>`,
/// `output/<key>`, by the first eight bytes of its key, as it names them.
fn named_for(directory: &Indexed<'_>, machine: &MachineName, stream: &str) -> bool {
    let Some(member) = directory.member(machine) else { return false };
    let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    stream
        .split_once('/')
        .is_some_and(|(kind, name)| matches!(kind, "activity" | "usage" | "jobs" | "output") && name == named)
}

/// Whether `machine` may hold any stream of the scope (design §2.3, §6.6),
/// an error where who reads it cannot be told now (AE04).
pub(crate) fn may_hold_known(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> std::io::Result<bool> {
    Ok(hold_known(directory, machine, scope)? != Hold::No)
}

/// Whether `machine` may hold `stream` of the scope (design §2.3): the
/// stream's class names who holds it, and a machine shared whole holds as a
/// copy only what each of its holders reads.
pub(crate) fn holds(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, stream: &str) -> bool {
    let descriptor = &scope.descriptor;
    let Some(class) = descriptor.class(stream) else { return false };
    // A project's ledger in an account's records: of a project it reads
    // now, under the machines it may work on now (§29, 2.4).
    if let Readers::Account { account } = &descriptor.readers
        && let Some(name) = stream.strip_prefix("ledger/")
        && !super::ledger_views::holds(directory.all, account, name)
    {
        return false;
    }
    holds_as(directory, machine, scope, class, Some(stream))
}

/// Whether `machine` may hold some stream of the class `class` of the scope:
/// what a listing of what it holds names (§6.6).
pub(crate) fn holds_class(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, class: &StreamClass) -> bool {
    holds_as(directory, machine, scope, class, None)
}

fn holds_as(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope, class: &StreamClass, stream: Option<&str>) -> bool {
    if !keeps_ledger(directory, machine, scope) {
        return false;
    }
    match (hold(directory, machine, scope), class.holders) {
        (Hold::Own | Hold::Copy, _) => true,
        (Hold::Made, Holders::Readers) => true,
        (Hold::Named, Holders::Readers) => stream.is_none_or(|stream| named_for(directory, machine, stream)),
        _ => false,
    }
}

/// Whether `machine` may hold a project's ledger, its entries whole as
/// their authors signed them: only a machine that keeps the project's
/// bodies does, as the ledger was copied before (L11, NN06). Any other
/// scope is not judged here.
fn keeps_ledger(_directory: &Indexed<'_>, _machine: &MachineName, scope: &Scope) -> bool {
    // A project's ledger names where its machines are, which not every
    // reader of the project may know: one described before is held by none
    // until its entries are kept apart by machine (M09, N17, O10).
    !matches!(&scope.descriptor.readers, Readers::Project { .. })
}

/// The scope `key`, if the hub described it.
pub(crate) fn scope(assets: &Assets, key: &str) -> Result<Option<Scope>, String> {
    // One deleted with its conversation is no scope, whatever row a write
    // that crossed its deletion left (`deleted`).
    if assets.was_deleted(key) {
        return Ok(None);
    }
    stored(assets, key)
}

/// The scope `key` as its row stands, deleted or not: for deleting what a
/// purge marked deleted as it was described (`deleted::purge_key`).
pub(crate) fn stored(assets: &Assets, key: &str) -> Result<Option<Scope>, String> {
    let key = key.to_string();
    let found: Option<Row> = assets
        .control
        .read_now(move |c| {
            Ok(c.prepare_cached("SELECT owner, descriptor, word, source FROM asset_scopes WHERE key = ?1")?
                .query_row([&key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .optional()?)
        })
        .map_err(|error| error.to_string())?;
    let Some((owner, descriptor, mut word, source)) = found else { return Ok(None) };
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
    Ok(Some(Scope { descriptor: read, fact: Fact { fact: Bytes(descriptor), word: Bytes(word) }, owner, bodies, source }))
}

/// The owner library of `scope`, with the scope installed in it: a scope
/// described just before a stop is installed at its first use.
pub(crate) fn library(assets: &Assets, scope: &Scope) -> Result<Library, String> {
    let key = &scope.descriptor.scope;
    // One deleted since it was read is installed in no library again.
    if assets.was_deleted(key) {
        return Err(format!("the scope {key} was deleted with its conversation"));
    }
    let library = assets.library(&scope.owner).map_err(|error| error.to_string())?;
    if library.holding(key).map_err(|error| error.to_string())?.is_none() {
        library.install(key, scope.fact.fact.0.clone(), scope.descriptor.seq as i64).map_err(|error| error.to_string())?;
        // Deleted while it was installed: the deletion let go of it before
        // this put it back, so it is let go of again.
        if assets.was_deleted(key) {
            library.revoke(key).map_err(|error| error.to_string())?;
            return Err(format!("the scope {key} was deleted with its conversation"));
        }
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
        Readers::Account { account } | Readers::Working { account, .. } => account.clone(),
        Readers::Hub => return Ok(Owner::Hub),
    };
    let opened = directory.account(&account).ok_or_else(|| format!("there is no account {account}"))?.opened;
    Ok(Owner::Account(account.to_string(), opened.as_millisecond()))
}

/// Describes the scope `key` that a device of `machine` begins (design
/// §2.1): one of the accounts it acts for must be among its readers, and a
/// conversation's openers among those accounts; each opener who it was as
/// the scope was begun, where `said` says (`fastctx_types::assets::BORN`).
/// The descriptor is signed, kept as a control fact and witnessed before it
/// is answered; the same description again is answered with the descriptor
/// kept.
#[allow(clippy::too_many_arguments)]
pub(crate) fn describe(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &str,
    readers: Readers,
    classes: Vec<StreamClass>,
    said: &BTreeMap<String, String>,
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
    if let Readers::Working { machine: on, .. } = &readers
        && directory.canonical(on) != directory.canonical(machine)
    {
        return Err("what an account does on a machine is described by that machine".into());
    }
    // Described only with its kind's classes: no device widens who holds it
    // (AS09).
    if classes != fastctx_types::assets::classes_for(&readers, key) {
        return Err(format!("the scope {key} is described with its own classes"));
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
    // A project's ledger names where its machines are, which not every
    // reader of the project may know: each reader reads it in its own
    // records, as it may know it, and no project's scope is described
    // (M09, N17, §29 2.4).
    if matches!(readers, Readers::Project { .. }) {
        return Err("a project's ledger is read in each reader's own records, not in a scope of its own".into());
    }
    let principal = principal(directory, machine);
    let born = identities(directory, &readers)?;
    // Begun for an account of a name opened again since: described for no
    // later account of that name.
    if let Some((opener, _)) = said.iter().find(|(opener, was)| born.get(*opener).is_some_and(|now| now != *was)) {
        return Err(format!("{}{opener}", fastctx_types::assets::BEGUN_FOR_ANOTHER));
    }
    if let Readers::Conversation { openers, .. } = &readers
        && !openers.iter().all(|opener| principal.contains(opener))
    {
        return Err("a conversation's openers are accounts this machine acts for".into());
    }
    let bodies = matches!(readers, Readers::Body { .. }).then(|| assets.bodies.clone());
    if let Readers::Body { body, opener } = &readers {
        // A body's scope is described by its machine, as that body — the
        // body it is now, or one the hub kept it as before its owner changed
        // (§29, 2.1) — and opened by an account it acts for.
        if !assets.bodies.is_of(directory.all, body, machine).map_err(|error| error.to_string())? {
            return Err("a body's scope is described by the machine that is that body".into());
        }
        // Its log, opened by its owner, though none is logged in there (a
        // machine its owner manages, PP10, N08).
        let owner = directory.member(machine).and_then(|member| member.home.clone());
        let own_log = machines_own(key) && owner.as_ref() == Some(opener);
        if !own_log && !principal.contains(opener) {
            return Err("a body's opener is an account this machine acts for".into());
        }
        assets.bodies.known(directory.all, body, directory.canonical(machine)).map_err(|error| error.to_string())?;
    }
    let own_log = matches!(&readers, Readers::Body { body, .. }
        if machines_own(key) && super::bodies::body_of(directory.all, machine).as_deref() == Some(body.as_str()));
    if !own_log && !principal.iter().any(|account| reads(directory, account, &readers, &born, bodies.as_deref())) {
        return Err("none of this machine's accounts reads that scope".into());
    }
    let owner = owner_of(directory, &readers)?;
    // A conversation born assisting an account: the machine describing it
    // runs it, and is the one machine that holds it as what it makes
    // (§28.47).
    let source = match &readers {
        Readers::Conversation { born_on: None, .. } => {
            Some(machine_now(directory, machine).ok_or_else(|| format!("{machine} is not a machine of this network"))?)
        }
        _ => None,
    };
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
    let (key_text, owner_key, stored, sourced) = (key.to_string(), owner.key(), bytes.clone(), source.clone());
    // A conversation described into a body: its records are the body's
    // readers' from now on, kept with the description (§29, 2.6).
    let into_body = match &descriptor.readers {
        Readers::Body { body, opener } if !machines_own(key) => directory
            .account(opener)
            .map(|found| (body.clone(), opener.clone(), found.opened, key[prefix.len()..].to_string())),
        _ => None,
    };
    let keeping = into_body.clone();
    let (deleted, stamp) = (assets.deleted_scopes().map_err(|error| error.to_string())?, super::deleted::stamp(key));
    let (seq, head) = assets
        .control
        .write_now(256 + bytes.len(), move |c| {
            // Never one deleted with its conversation, the deletion begun
            // since this was asked (`deleted`).
            if deleted.lock().expect("deleted scopes").contains(&stamp) {
                return Ok(None);
            }
            // Never into a library being deleted (design §6.2).
            let added = c
                .prepare_cached(
                    "INSERT INTO asset_scopes (key, owner, descriptor, word, seq, source)
                     SELECT ?1, ?2, ?3, x'', 0, ?4 WHERE NOT EXISTS (SELECT 1 FROM asset_deleting WHERE owner = ?2)",
                )?
                .execute(params![key_text, owner_key, stored, sourced])?;
            if added == 0 {
                return Ok(None);
            }
            if let Some((body, opener, opened, resident)) = &keeping {
                super::bodies::Bodies::keep_conversation_in(c, body, opener, *opened, resident)?;
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
    // Kept in memory as it is in the database, whether or not the witness
    // takes the head now: described again, the kept scope is answered with
    // nothing written (§29, 2.6).
    if let Some((body, opener, opened, resident)) = &into_body {
        assets.bodies.kept_conversation(body, opener, *opened, resident);
    }
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
    let kept = Scope { descriptor, fact: Fact { fact: Bytes(bytes), word: Bytes(word) }, owner, bodies, source };
    library(assets, &kept)?;
    // What the hub's records hold of it goes into it (design §28.17).
    assets.described.lock().expect("described scopes").push(key.to_string());
    Ok(kept.fact)
}
