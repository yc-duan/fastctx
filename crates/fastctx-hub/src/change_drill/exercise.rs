//! Local persistence and failure checks outside the benchmark's timed region.

use std::sync::Arc;

use fastctx_net::{
    directory::ActsFor,
    wire::{Change, Told},
};
use jiff::{SignedDuration, Timestamp};

use super::{account, machine};
use crate::{catalog::Catalog, origin::Origin};

pub(super) fn run(catalog: &Catalog, origin: &Origin, machines: usize) -> anyhow::Result<()> {
    let target = account(machines / 2 - 1);
    let old = catalog.read();
    let original = old.book.directory.account(&target).expect("account").history;
    {
        let mut store = catalog.edit();
        let change = Change::Group { machine: origin.name.clone(), group: Some("drill".into()) };
        crate::manage::commit(&mut store, origin, &target, change, |store, _, _| {
            Arc::make_mut(store.book.directory.members.last_mut().expect("last member")).group = Some("drill".into());
            Ok(Told::Done)
        })
        .map_err(anyhow::Error::msg)?;
    }
    let grouped = catalog.read();
    anyhow::ensure!(
        grouped.book.directory.members.last().expect("last member").group.as_deref() == Some("drill"),
        "group published"
    );
    anyhow::ensure!(old.book.directory.members.last().expect("old member").group.is_none(), "old group snapshot");
    super::verify::check(&old, &grouped, machines)?;
    {
        let mut store = catalog.edit();
        let before = store.book.clone();
        let result =
            crate::manage::commit(&mut store, origin, &target, Change::History { on: !original }, |store, _, _| {
                crate::manage::history(store, &target, !original)?;
                Err("injected apply failure".into())
            });
        anyhow::ensure!(
            result.as_ref().err().map(String::as_str) == Some("injected apply failure"),
            "apply failure reported"
        );
        anyhow::ensure!(store.book.directory == before.directory, "apply failure restored directory");
    }
    let database = catalog.read().shelf.0.clone();
    database.write_now(0, |c| {
        c.execute_batch("CREATE TRIGGER change_drill_fail BEFORE INSERT ON hub_rows WHEN NEW.kind='accounts' BEGIN SELECT RAISE(ABORT, 'injected row failure'); END;")?;
        Ok(())
    })?;
    let before_failure = catalog.read();
    {
        let mut store = catalog.edit();
        let result =
            crate::manage::commit(&mut store, origin, &target, Change::History { on: !original }, |store, _, _| {
                crate::manage::history(store, &target, !original)
            });
        anyhow::ensure!(result.is_err(), "durable failure reported");
    }
    let failed = catalog.read();
    anyhow::ensure!(failed.book.directory == before_failure.book.directory, "failed write not published");
    anyhow::ensure!(failed.book.machine_log.next == before_failure.book.machine_log.next, "failed audit not published");
    database.write_now(0, |c| {
        c.execute_batch("DROP TRIGGER change_drill_fail;")?;
        Ok(())
    })?;
    {
        let mut store = catalog.edit();
        crate::manage::commit(&mut store, origin, &target, Change::History { on: !original }, |store, _, _| {
            crate::manage::history(store, &target, !original)
        })
        .map_err(anyhow::Error::msg)?;
    }
    let recovered = catalog.read();
    anyhow::ensure!(
        recovered.book.directory.account(&target).expect("recovered account").history != original,
        "uncertain baseline repaired"
    );
    let persisted: fastctx_net::directory::Directory = crate::store::Store::read_directory(recovered.dir())?;
    anyhow::ensure!(persisted == recovered.book.directory, "recovery directory persisted");

    // Give two assistances reversed deadlines. Expiration keeps directory order,
    // and assistance ending retains its acting record for a later grant.
    let now = Timestamp::now();
    let due = now.checked_sub(SignedDuration::from_secs(1))?;
    {
        let mut store = catalog.edit();
        Arc::make_mut(&mut store.book.directory.assists[0]).until = Some(due);
        Arc::make_mut(&mut store.book.directory.assists[2]).until =
            Some(due.checked_sub(SignedDuration::from_secs(1))?);
        Arc::make_mut(&mut store.book.directory.assists[1]).until =
            Some(now.checked_add(SignedDuration::from_secs(60))?);
        store.book.directory.acting.push(Arc::new(ActsFor {
            machine: machine(machines / 2),
            account: account(1),
            since: now,
        }));
        store.save_book()?;
        let expected: Vec<_> =
            store.book.directory.assists.iter().filter(|assist| !assist.live(now)).cloned().collect();
        anyhow::ensure!(store.assist_ends.due(now) == expected, "deadline index preserves scan ordering");
        anyhow::ensure!(crate::assists::end_due(&mut store, now)?, "due assistance ended");
        anyhow::ensure!(
            store.book.directory.assists.iter().all(|assist| assist.live(now)),
            "only live assistance remains"
        );
        anyhow::ensure!(
            store
                .book
                .directory
                .acting
                .iter()
                .any(|acting| acting.machine == machine(machines / 2) && acting.account == account(1)),
            "assistance expiry retains acting"
        );
        anyhow::ensure!(
            !store.indexed().rights(&account(1), &machine(machines / 2)).agents,
            "expired assistance grants no authority"
        );
        anyhow::ensure!(!crate::assists::end_due(&mut store, now)?, "no due work on the next pass");
    }
    let final_snapshot = catalog.read();
    let persisted = crate::store::Store::read_directory(final_snapshot.dir())?;
    anyhow::ensure!(persisted == final_snapshot.book.directory, "expiry directory persisted");
    println!(
        "machines={machines} verified_group_patch=true verified_apply_rollback=true verified_failed_commit_recovery=true verified_expiration_order=true verified_expired_authority=true"
    );
    Ok(())
}
