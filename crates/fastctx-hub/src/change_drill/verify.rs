//! Cross-checks the benchmark's persisted state and reused read indexes.

use std::sync::Arc;

use fastctx_net::authorization::Action;
use fastctx_net::directory::Directory;

use super::{account, machine};
use crate::store::Snapshot;

pub(super) fn check(before: &Snapshot, after: &Snapshot, machines: usize) -> anyhow::Result<()> {
    let all = &after.book.directory;
    let loaded = Directory {
        accounts: after.shelf.rows("accounts")?.into_iter().map(|(_, _, row)| row).collect(),
        members: after.shelf.rows("members")?.into_iter().map(|(_, _, row)| row).collect(),
        shares: after.shelf.rows("shares")?.into_iter().map(|(_, _, row)| row).collect(),
        assists: after.shelf.rows("lends")?.into_iter().map(|(_, _, row)| row).collect(),
        plans: after.shelf.rows("plans")?.into_iter().map(|(_, _, row)| row).collect(),
        acting: after.shelf.rows("acting")?.into_iter().map(|(_, _, row)| row).collect(),
        projects: after.shelf.rows("projects")?.into_iter().map(|(_, _, row)| row).collect(),
    };
    anyhow::ensure!(*all == loaded, "every durable directory row matches its published value and order");
    let index = after.indexed();
    let old_index = before.indexed();
    let mut decisions = 0;
    for i in 0..256 {
        let who = account((i * 71) % (machines / 2));
        let from = machine((i * 31) % machines);
        let to = machine((i * 97 + 1) % machines);
        for action in [
            Action::Work(&to),
            Action::Login { machine: &to, agent: None, home: None },
            Action::Look(&to),
            Action::Manage(&to),
            Action::Hold(&to),
            Action::ActFor(&to),
            Action::Call { from: &from, to: &to, purpose: fastctx_types::access::Purpose::Hands },
            Action::Call { from: &from, to: &to, purpose: fastctx_types::access::Purpose::Agents },
        ] {
            anyhow::ensure!(
                all.authorize(&who, action) == index.authorize(&who, action),
                "index and scan: {who} {action:?}"
            );
            anyhow::ensure!(
                old_index.authorize(&who, action) == index.authorize(&who, action),
                "configuration preserves authorization: {who} {action:?}"
            );
            decisions += 1;
        }
    }
    anyhow::ensure!(Arc::ptr_eq(&before.index, &after.index), "configuration reuses the directory index");
    anyhow::ensure!(Arc::ptr_eq(&before.audience, &after.audience), "configuration reuses the presence audience");
    anyhow::ensure!(Arc::ptr_eq(&before.tokens, &after.tokens), "configuration reuses the token index");
    println!(
        "machines={machines} verified_durable_rows={} verified_authorization_decisions={decisions} frozen_snapshot=true shared_indexes=true",
        all.accounts.len()
            + all.members.len()
            + all.shares.len()
            + all.assists.len()
            + all.plans.len()
            + all.acting.len()
            + all.projects.len()
    );
    Ok(())
}
