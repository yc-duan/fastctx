//! What the hub tells an account, written into its records' scope (design
//! §14, §28.20): the machine changes it reads (`machine.audit`, stream
//! `audit`), each record as the machine log keeps it, by its number; and the
//! troubles it hears of (`account.notice`, stream `notices`), as raised and
//! as resolved; and the account's own settings the directory keeps — its
//! layer of configuration (`account.profile`, stream `profile`) and how its
//! models work together (`account.collaboration`, stream `collaboration`) —
//! each as it stands. The machine log is followed forward by number as it grows,
//! each account from the newest record its scope holds; a scope described
//! later is caught up the same way. The hub keeps the machine log for good
//! and its troubles a week, for views; an account's scope keeps both until
//! the account is deleted. Only the account opened when a record was made
//! reads it: a later one of the same name owns another scope.

use super::Assets;
use super::mirror::{account_scope, apply, writing};
use super::owners::Owner;
use super::scopes;
use fastctx_content::v2::{AccountCollaboration, AccountNotice, AccountProfile, Kind as _, KindRecord, MachineAudit};
use fastctx_content::{Anchor, Record as Item, State};
use fastctx_types::AccountName;
use fastctx_types::trouble::{Trouble, TroubleKind};
use jiff::Timestamp;
use std::sync::Arc;

/// The most records written in one go.
const A_BATCH: usize = 256;

/// The scope of `account`'s records, held here, when the account opened at
/// `opened` owns it.
fn scope_of(
    assets: &Assets,
    account: &AccountName,
    opened: Timestamp,
) -> Result<Option<(scopes::Scope, fastctx_content::v2::Library)>, String> {
    let Some(scope) = scopes::scope(assets, &account_scope(assets, account))? else { return Ok(None) };
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

/// Writes the machine changes `account`, opened at `opened`, reads that its
/// scope lacks, oldest first; `whole` for a super administrator, who reads
/// every record but those of conversations and projects. Call on a thread
/// that may block.
pub(crate) fn audit(
    assets: &Assets,
    database: &fastctx_storage::Database,
    account: &AccountName,
    opened: Timestamp,
    whole: bool,
) -> Result<(), String> {
    let Some((scope, library)) = scope_of(assets, account, opened)? else { return Ok(()) };
    let key = scope.descriptor.scope.clone();
    let error = |error: fastctx_content::Error| error.to_string();
    let mut readers = vec![crate::store::log_store::account(account, opened)];
    if whole {
        readers.push("whole".into());
    }
    let _writing = writing();
    let newest = library.view_page(&key, MachineAudit::STREAM, &Anchor::Latest, 1, u32::MAX).map_err(error)?;
    let mut after = newest
        .entries
        .last()
        .and_then(|entry| std::str::from_utf8(&entry.item).ok().and_then(|hex| u64::from_str_radix(hex, 16).ok()));
    loop {
        let rows = crate::store::log_store::readable_after(database, &readers, after, A_BATCH)
            .map_err(|error| error.to_string())?;
        let Some(&(last, _)) = rows.last() else { return Ok(()) };
        let records = rows
            .into_iter()
            .map(|(id, body)| KindRecord {
                kind: MachineAudit::KIND,
                schema: 1,
                record: Item { item: audit_item(id), order: audit_item(id), state: State::Is { body, objects: Vec::new() } },
            })
            .collect();
        apply(assets, &scope, &library, MachineAudit::STREAM, records)?;
        after = Some(last);
    }
}

/// The item a trouble is kept by: when it was raised and its number, as a
/// hub that kept none of its troubles counts them from one again.
fn notice_item(trouble: &Trouble) -> Vec<u8> {
    format!("{:016x}{:016x}", trouble.at.as_millisecond().max(0), trouble.id).into_bytes()
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
    let _writing = writing();
    for batch in troubles.chunks(A_BATCH) {
        let mut records = Vec::new();
        for trouble in batch.iter().filter(|trouble| trouble.kind != TroubleKind::Violations) {
            let item = notice_item(trouble);
            let body = serde_json::to_vec(trouble).map_err(|error| error.to_string())?;
            if library.item(&key, AccountNotice::STREAM, &item).map_err(error)?.is_some_and(|entry| entry.body == body) {
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

/// What the directory keeps of `account`.
pub(crate) fn kept_of(account: &fastctx_net::directory::Account) -> Kept {
    (
        account.profile.as_ref().and_then(|profile| serde_json::to_vec(profile).ok()),
        account.collab.as_ref().and_then(|collab| serde_json::to_vec(collab).ok()),
    )
}

/// Writes the account's settings the directory keeps into its scope where
/// they are not there as they stand: one it no longer has, let go. Call on a
/// thread that may block.
pub(crate) fn settings(assets: &Assets, account: &AccountName, opened: Timestamp, kept: &Kept) -> Result<(), String> {
    let Some((scope, library)) = scope_of(assets, account, opened)? else { return Ok(()) };
    let key = scope.descriptor.scope.clone();
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = writing();
    for (stream, kind, body) in [
        (AccountProfile::STREAM, AccountProfile::KIND, &kept.0),
        (AccountCollaboration::STREAM, AccountCollaboration::KIND, &kept.1),
    ] {
        let item = stream.as_bytes().to_vec();
        let now = library.item(&key, stream, &item).map_err(error)?;
        let state = match (body, now) {
            (Some(body), Some(entry)) if entry.body == *body => continue,
            (None, None) => continue,
            (Some(body), _) => State::Is { body: body.clone(), objects: Vec::new() },
            (None, Some(_)) => State::Gone,
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
            }
        }
    });
}

/// Writes what the hub tells `account`, opened at `opened`, that its scope
/// lacks: as the scope is described.
pub(crate) async fn caught_up(hub: &Arc<crate::Hub>, account: &AccountName, opened: Timestamp) {
    let (whole, database, kept) = {
        let store = hub.store();
        let directory = &store.book.directory;
        let kept = directory.account(account).filter(|found| found.opened == opened).map(kept_of);
        (directory.is_super(account), store.shelf.0.clone(), kept)
    };
    let troubles = hub.troubles.told_to(account, opened);
    let (assets, log, account) = (hub.assets.clone(), hub.log.clone(), account.clone());
    let _ = fastctx_platform::task::spawn_blocking(move || {
        if let Err(error) = audit(&assets, &database, &account, opened, whole) {
            log.line(format_args!("writing the machine log of {account} into the asset library: {error}"));
        }
        if let Err(error) = notices(&assets, &account, opened, &troubles) {
            log.line(format_args!("writing the troubles of {account} into the asset library: {error}"));
        }
        if let Some(kept) = kept
            && let Err(error) = settings(&assets, &account, opened, &kept)
        {
            log.line(format_args!("writing the settings of {account} into the asset library: {error}"));
        }
    })
    .await;
}

/// Follows the machine log for as long as the hub runs: each time it grows,
/// each account's scope is written on from what it holds, and its settings
/// as they stand — every change of them is logged. Once at the start, every
/// account's troubles too.
pub(crate) async fn follow(hub: &Arc<crate::Hub>) -> std::io::Result<()> {
    let mut seen = None;
    let mut first = true;
    loop {
        let notified = hub.machine_log_changed.notified();
        let (accounts, database) = {
            let store = hub.store();
            let directory = &store.book.directory;
            let accounts: Vec<(AccountName, Timestamp, bool, Kept)> = directory
                .accounts
                .iter()
                .map(|account| (account.name.clone(), account.opened, directory.is_super(&account.name), kept_of(account)))
                .collect();
            (accounts, store.shelf.0.clone())
        };
        let troubles: Vec<Vec<Trouble>> = if first {
            accounts.iter().map(|(account, opened, _, _)| hub.troubles.told_to(account, *opened)).collect()
        } else {
            Vec::new()
        };
        let (assets, log) = (hub.assets.clone(), hub.log.clone());
        let newest = fastctx_platform::task::spawn_blocking(move || {
            let mut newest = crate::store::log_store::newest(&database).ok().flatten();
            for (account, opened, _, kept) in &accounts {
                if let Err(error) = settings(&assets, account, *opened, kept) {
                    log.line(format_args!("writing the settings of {account} into the asset library: {error}"));
                }
            }
            if newest.is_some() && newest != seen {
                for (account, opened, whole, _) in &accounts {
                    if let Err(error) = audit(&assets, &database, account, *opened, *whole) {
                        log.line(format_args!("writing the machine log of {account} into the asset library: {error}"));
                        // Looked at again as the log next grows.
                        newest = seen;
                    }
                }
            }
            for ((account, opened, _, _), troubles) in accounts.iter().zip(&troubles) {
                if let Err(error) = notices(&assets, account, *opened, troubles) {
                    log.line(format_args!("writing the troubles of {account} into the asset library: {error}"));
                }
            }
            newest
        })
        .await
        .unwrap_or(seen);
        seen = newest;
        first = false;
        notified.await;
    }
}
