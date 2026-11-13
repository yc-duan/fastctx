//! What the hub tells an account, written into its records' scope (design
//! §14, §28.20): the machine changes it reads (`machine.audit`, stream
//! `audit`), each record as the machine log keeps it, by its number; the
//! troubles it hears of (`account.notice`, stream `notices`), as raised and
//! as resolved; and the account's own settings the directory keeps — its
//! layer of configuration (`account.profile`, stream `profile`) and how its
//! models work together (`account.collaboration`, stream `collaboration`) —
//! each as it stands. The machine log is followed forward by number as it
//! grows, each account from the newest record its scope holds, and the last
//! few it holds looked at again against the log, which a write whose end
//! was not known may have replaced (AR11); a scope described later is caught
//! up the same way. Only the records the account itself reads go in: what a
//! super administrator reads of the whole network stays with the log's own
//! reading, which judges who is one now (AR09, L13). The hub keeps the
//! machine log for good and its troubles a week, for views; an account's
//! scope keeps both until the account is deleted. Only the account opened
//! when a record was made reads it: a later one of the same name owns
//! another scope.

use super::Assets;
use super::mirror::{account_scope, apply, fitted, writing};
use super::owners::Owner;
use super::scopes;
use fastctx_content::v2::{AccountCollaboration, AccountNotice, AccountProfile, Kind as _, KindRecord, MachineAudit};
use fastctx_content::{Anchor, Record as Item, State};
use fastctx_types::AccountName;
use fastctx_types::trouble::{Trouble, TroubleKind};
use jiff::Timestamp;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The most records written in one go, the scope held for that long only.
const A_BATCH: usize = 256;
/// How many of the newest records a scope holds are looked at again, a page
/// at a time, and how many pages back at most in one look.
const LOOKED_AGAIN: u32 = 32;
const VERIFIED_PAGES: usize = 64;

/// Whether a write of what the hub tells failed since the last look: then
/// the next minute's look writes it all again (AR15).
static FAILED: AtomicBool = AtomicBool::new(false);

/// The scope of `account`'s records, held here, when the account opened at
/// `opened` owns it.
fn scope_of(
    assets: &Assets,
    account: &AccountName,
    opened: Timestamp,
) -> Result<Option<(scopes::Scope, fastctx_content::v2::Library)>, String> {
    let Some(scope) = scopes::scope(assets, &account_scope(assets, account, opened))? else { return Ok(None) };
    if !matches!(&scope.owner, Owner::Account(name, at) if name == account.as_str() && *at == opened.as_millisecond())
    {
        return Ok(None);
    }
    let library = scopes::library(assets, &scope)?;
    Ok(Some((scope, library)))
}

/// The item a machine log record is kept by: its number.
fn audit_item(id: u64) -> Vec<u8> {
    format!("{id:016x}").into_bytes()
}

fn audit_id(item: &[u8]) -> Option<u64> {
    std::str::from_utf8(item).ok().and_then(|hex| u64::from_str_radix(hex, 16).ok())
}

/// The record `id`, its body as `body`, as it goes in.
fn audit_record(
    library: &fastctx_content::v2::Library,
    id: u64,
    body: Option<Vec<u8>>,
) -> Result<(KindRecord, Option<fastctx_content::v2::Lease>), String> {
    let (state, lease) = match body {
        Some(body) => {
            let (body, objects, lease) = fitted(library, body)?;
            (State::Is { body, objects }, lease)
        }
        None => (State::Gone, None),
    };
    let record = Item { item: audit_item(id), order: audit_item(id), state };
    Ok((KindRecord { kind: MachineAudit::KIND, schema: 1, record }, lease))
}

/// Whether `entry` holds `body` as it is, or as its one object.
fn holds(entry: &fastctx_content::v2::Entry, body: &[u8]) -> bool {
    match entry.objects.first() {
        None => entry.body == body,
        Some(object) => {
            use sha2::Digest as _;
            object.size == body.len() as u64 && object.hash == <[u8; 32]>::from(sha2::Sha256::digest(body))
        }
    }
}

/// Puts right the records `entries` of the scope holds, numbered `ids`, as
/// the log has them now: whether any was otherwise.
fn put_right(
    assets: &Assets,
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    database: &fastctx_storage::Database,
    readers: &[String],
    entries: &[fastctx_content::v2::Entry],
    ids: &[u64],
) -> Result<bool, String> {
    let now = crate::store::log_store::readable_of(database, readers, ids).map_err(|error| error.to_string())?;
    let (mut records, mut _leases) = (Vec::new(), Vec::new());
    for entry in entries {
        let Some(id) = audit_id(&entry.item) else { continue };
        let body = now.get(&id).cloned();
        if body.as_deref().is_some_and(|body| holds(entry, body)) {
            continue;
        }
        let (record, lease) = audit_record(library, id, body)?;
        records.push(record);
        _leases.extend(lease);
    }
    let fixed = !records.is_empty();
    apply(assets, scope, library, MachineAudit::STREAM, records)?;
    Ok(fixed)
}

/// Writes the machine changes `account`, opened at `opened`, reads that its
/// scope lacks, oldest first, and puts right the newest it holds where the
/// log has them otherwise now. Call on a thread that may block.
pub(crate) fn audit(
    assets: &Assets,
    database: &fastctx_storage::Database,
    account: &AccountName,
    opened: Timestamp,
) -> Result<(), String> {
    let Some((scope, library)) = scope_of(assets, account, opened)? else { return Ok(()) };
    let key = scope.descriptor.scope.clone();
    let error = |error: fastctx_content::Error| error.to_string();
    let readers = vec![crate::store::log_store::account(account, opened)];
    // The newest it holds, as the log has them now (AR11), and on back a
    // page at a time while a page held anything the log has otherwise: a
    // recovery longer than a page is put right whole (AS04).
    let mut anchor = Anchor::Latest;
    for _ in 0..VERIFIED_PAGES {
        let _writing = writing(&key);
        let newest = library.view_page(&key, MachineAudit::STREAM, &anchor, LOOKED_AGAIN, u32::MAX).map_err(error)?;
        let Some(oldest) = newest.older.clone() else {
            if newest.entries.is_empty() {
                break;
            }
            // The first page: nothing older to look at.
            let ids: Vec<u64> = newest.entries.iter().filter_map(|entry| audit_id(&entry.item)).collect();
            put_right(assets, &scope, &library, database, &readers, &newest.entries, &ids)?;
            break;
        };
        let ids: Vec<u64> = newest.entries.iter().filter_map(|entry| audit_id(&entry.item)).collect();
        if !put_right(assets, &scope, &library, database, &readers, &newest.entries, &ids)? {
            break;
        }
        anchor = Anchor::Before(oldest);
    }
    loop {
        let _writing = writing(&key);
        let newest = library.view_page(&key, MachineAudit::STREAM, &Anchor::Latest, 1, u32::MAX).map_err(error)?;
        let after = newest.entries.last().and_then(|entry| audit_id(&entry.item));
        let rows = crate::store::log_store::readable_after(database, &readers, after, A_BATCH)
            .map_err(|error| error.to_string())?;
        if rows.is_empty() {
            return Ok(());
        }
        let (mut records, mut _leases) = (Vec::new(), Vec::new());
        for (id, body) in rows {
            let (record, lease) = audit_record(&library, id, Some(body))?;
            records.push(record);
            _leases.extend(lease);
        }
        apply(assets, &scope, &library, MachineAudit::STREAM, records)?;
    }
}

/// The item a trouble is kept by: when it was raised and its number, as a
/// hub that kept none of its troubles counts them from one again.
fn notice_item(trouble: &Trouble) -> Vec<u8> {
    format!("{:016x}{:016x}", trouble.at.as_millisecond().max(0), trouble.id).into_bytes()
}

/// Whether a notice kept says its trouble was resolved: once resolved it
/// stays so, whatever late word of its raising comes after (AR12, L15).
fn resolved(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .is_some_and(|value| value.get("resolved").is_some_and(|resolved| !resolved.is_null()))
}

/// Writes `troubles`, which the account opened at `opened` hears of, into
/// its scope where they are not there as they are. Call on a thread that may
/// block.
pub(crate) fn notices(
    assets: &Assets,
    account: &AccountName,
    opened: Timestamp,
    troubles: &[Trouble],
) -> Result<(), String> {
    let Some((scope, library)) = scope_of(assets, account, opened)? else { return Ok(()) };
    let key = scope.descriptor.scope.clone();
    let error = |error: fastctx_content::Error| error.to_string();
    for batch in troubles.chunks(A_BATCH) {
        let _writing = writing(&key);
        let mut records = Vec::new();
        for trouble in batch.iter().filter(|trouble| trouble.kind != TroubleKind::Violations) {
            let item = notice_item(trouble);
            let body = serde_json::to_vec(trouble).map_err(|error| error.to_string())?;
            if let Some(entry) = library.item(&key, AccountNotice::STREAM, &item).map_err(error)?
                && (entry.body == body || (resolved(&entry.body) && trouble.resolved.is_none()))
            {
                continue;
            }
            records.push(KindRecord {
                kind: AccountNotice::KIND,
                schema: 1,
                record: Item { item: item.clone(), order: item, state: State::Is { body, objects: Vec::new() } },
            });
        }
        apply(assets, &scope, &library, AccountNotice::STREAM, records)?;
    }
    Ok(())
}

/// What the directory keeps of an account for its scope: its profile and
/// its collaboration, each as JSON, none where it has none.
pub(crate) type Kept = (Option<Vec<u8>>, Option<Vec<u8>>);

/// What the directory keeps of `account`: its profile without what an MCP
/// server at an address is asked with — the values of its headers, and of
/// its address the user and password, query and fragment — which stay on
/// the hub alone, given to a machine that runs the server as before, never
/// into a replica (user 2026-11-11; M01, AT01).
pub(crate) fn kept_of(account: &fastctx_net::directory::Account) -> Kept {
    (
        account.profile.as_ref().map(without_secrets).and_then(|profile| serde_json::to_vec(&profile).ok()),
        account.collab.as_ref().and_then(|collab| serde_json::to_vec(collab).ok()),
    )
}

/// The profile with each server at an address bare: its headers' names
/// with empty values, its address's scheme, host and path. Nothing else of
/// the profile is touched: what an account sets for its sessions is chosen
/// from lists that hold no secret, and a server's or a setting's name is
/// only a name (AT09).
fn without_secrets(profile: &fastctx_types::profile::Profile) -> fastctx_types::profile::Profile {
    let mut profile = profile.clone();
    for entry in profile.servers.values_mut() {
        if let fastctx_types::profile::Entry::Remote(remote) = entry {
            remote.headers.values_mut().for_each(String::clear);
            remote.url = bare(&remote.url);
        }
    }
    profile
}

/// An address without the user and password before its host, its query
/// and its fragment.
fn bare(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").map_or(("", url), |(scheme, rest)| (scheme, rest));
    let rest = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if scheme.is_empty() { format!("{host}{path}") } else { format!("{scheme}://{host}{path}") }
}

/// Writes the account's settings the directory keeps into its scope where
/// they are not there as they stand: one it no longer has, let go. Call on a
/// thread that may block.
pub(crate) fn settings(assets: &Assets, account: &AccountName, opened: Timestamp, kept: &Kept) -> Result<(), String> {
    let Some((scope, library)) = scope_of(assets, account, opened)? else { return Ok(()) };
    let key = scope.descriptor.scope.clone();
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = writing(&key);
    for (stream, kind, body) in [
        (AccountProfile::STREAM, AccountProfile::KIND, &kept.0),
        (AccountCollaboration::STREAM, AccountCollaboration::KIND, &kept.1),
    ] {
        let item = stream.as_bytes().to_vec();
        let now = library.item(&key, stream, &item).map_err(error)?;
        let (state, _lease) = match (body, now) {
            (Some(body), Some(entry)) if holds(&entry, body) => continue,
            (None, None) => continue,
            (Some(body), _) => {
                let (body, objects, lease) = fitted(&library, body.clone())?;
                (State::Is { body, objects }, lease)
            }
            (None, Some(_)) => (State::Gone, None),
        };
        let record = KindRecord { kind, schema: 1, record: Item { item: item.clone(), order: item, state } };
        apply(assets, &scope, &library, stream, vec![record])?;
    }
    Ok(())
}

/// Writes `trouble`, as raised or resolved just now, into the scope of each
/// account that hears of it.
pub(crate) fn told(hub: &crate::Hub, trouble: Trouble, to: Vec<(AccountName, Timestamp)>) {
    let (assets, log) = (hub.assets.clone(), hub.log.clone());
    fastctx_platform::task::spawn_blocking(move || {
        for (account, opened) in to {
            if let Err(error) = notices(&assets, &account, opened, std::slice::from_ref(&trouble)) {
                log.line(format_args!("writing trouble {} for {account} into the asset library: {error}", trouble.id));
                FAILED.store(true, Ordering::SeqCst);
            }
        }
    });
}

/// Writes what the hub tells `account`, opened at `opened`, that its scope
/// lacks: as the scope is described.
pub(crate) async fn caught_up(hub: &Arc<crate::Hub>, account: &AccountName, opened: Timestamp) {
    let (database, kept) = {
        let store = hub.store();
        let directory = &store.book.directory;
        let kept = directory.account(account).filter(|found| found.opened == opened).map(kept_of);
        (store.shelf.0.clone(), kept)
    };
    let accounts = vec![(account.clone(), opened, kept.unwrap_or_default())];
    let troubles = vec![hub.troubles.told_to(account, opened)];
    pass(hub, database, accounts, Some(troubles)).await;
}

/// One look at every account's scope: its settings and audit, and, when
/// given, its troubles.
async fn pass(
    hub: &Arc<crate::Hub>,
    database: fastctx_storage::Database,
    accounts: Vec<(AccountName, Timestamp, Kept)>,
    troubles: Option<Vec<Vec<Trouble>>>,
) {
    let (assets, log) = (hub.assets.clone(), hub.log.clone());
    let _ = fastctx_platform::task::spawn_blocking(move || {
        for (at, (account, opened, kept)) in accounts.iter().enumerate() {
            let mut failed = Vec::new();
            if let Err(error) = settings(&assets, account, *opened, kept) {
                failed.push(format!("settings: {error}"));
            }
            if let Err(error) = audit(&assets, &database, account, *opened) {
                failed.push(format!("machine log: {error}"));
            }
            if let Some(troubles) = troubles.as_ref().and_then(|troubles| troubles.get(at))
                && let Err(error) = notices(&assets, account, *opened, troubles)
            {
                failed.push(format!("troubles: {error}"));
            }
            if !failed.is_empty() {
                log.line(format_args!("writing what the hub tells {account} into the asset library: {}", failed.join("; ")));
                FAILED.store(true, Ordering::SeqCst);
            }
        }
    })
    .await;
}

/// The accounts as the directory has them now, with what it keeps of each.
type Accounts = Vec<(AccountName, Timestamp, Kept)>;

fn accounts_now(hub: &crate::Hub) -> (fastctx_storage::Database, Accounts) {
    let store = hub.store();
    let accounts =
        store.book.directory.accounts.iter().map(|account| (account.name.clone(), account.opened, kept_of(account))).collect();
    (store.shelf.0.clone(), accounts)
}

/// Writes again what failed to be written since the last minute (AR15).
pub(crate) async fn again(hub: &Arc<crate::Hub>) {
    if !FAILED.swap(false, Ordering::SeqCst) {
        return;
    }
    let (database, accounts) = accounts_now(hub);
    let troubles = accounts.iter().map(|(account, opened, _)| hub.troubles.told_to(account, *opened)).collect();
    pass(hub, database, accounts, Some(troubles)).await;
}

/// Follows the machine log for as long as the hub runs: each time it
/// changes, each account's scope is written on from what it holds, and its
/// settings as they stand — every change of them is logged. Once at the
/// start, every account's troubles too.
pub(crate) async fn follow(hub: &Arc<crate::Hub>) -> std::io::Result<()> {
    let mut first = true;
    loop {
        let notified = hub.machine_log_changed.notified();
        let (database, accounts) = accounts_now(hub);
        let troubles = first
            .then(|| accounts.iter().map(|(account, opened, _)| hub.troubles.told_to(account, *opened)).collect());
        pass(hub, database, accounts, troubles).await;
        first = false;
        notified.await;
    }
}

#[cfg(test)]
mod bare_tests {
    #[test]
    fn an_address_keeps_no_secret() {
        assert_eq!(super::bare("https://user:pw@example.test/mcp?token=x#y"), "https://example.test/mcp");
        assert_eq!(super::bare("https://example.test:8443/a/b"), "https://example.test:8443/a/b");
        assert_eq!(super::bare("http://a@b@example.test#f"), "http://example.test");
        assert_eq!(super::bare("https://example.test/p@q?x"), "https://example.test/p@q");
    }
}
