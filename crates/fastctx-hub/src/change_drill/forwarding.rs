//! A forwarding hub has a progress message even when directory authority is unchanged.
use crate::Hub;
use fastctx_net::wire::Change;
use serde_json::Value;
use std::path::Path;
use tokio::sync::mpsc;

type Receiver = mpsc::UnboundedReceiver<crate::moving::frames::ToOld>;
fn activate(hub: &Hub) -> anyhow::Result<Receiver> {
    {
        let mut store = hub.edit_store();
        store.moves[0].old = fastctx_types::hub_move::OldPlace::Forwarding;
        store.save_moves()?;
    }
    let (out, receiver) = mpsc::unbounded_channel();
    hub.moving.olds.lock().expect("old hubs").insert("probe-move".into(), out);
    Ok(receiver)
}
fn take(receiver: &mut Receiver) -> anyhow::Result<Vec<Value>> {
    let mut frames = Vec::new();
    while let Ok(frame) = receiver.try_recv() {
        frames.push(serde_json::to_value(frame)?);
    }
    Ok(frames)
}
pub(super) fn run(root: &Path, base: &Path) -> anyhow::Result<()> {
    let left_dir = root.join("forwarding-narrow");
    let right_dir = root.join("forwarding-full");
    super::equivalence::copy_dir(base, &left_dir)?;
    super::equivalence::copy_dir(base, &right_dir)?;
    for dir in [&left_dir, &right_dir] {
        let mut store = crate::Store::open(dir)?;
        let member = store.book.directory.members[0].clone();
        let account = store.book.directory.accounts[0].clone();
        let mut book = crate::Book::default();
        book.directory.members.push(member.clone());
        book.directory.accounts.push(account);
        store.book = book.into();
        store.hub.package_targets.retain(|key, _| *key == member.key);
        store.moves[0].members = [(member.name.clone(), true)].into_iter().collect();
        store.save_book()?;
        store.save_hub()?;
        store.save_moves()?;
        drop(store);
        super::equivalence::wait_closed(dir)?;
    }
    let left = super::local_hub::open(&left_dir)?;
    let right = super::local_hub::open(&right_dir)?;
    let mut left_out = activate(&left)?;
    let mut right_out = activate(&right)?;
    left.configuration_changed();
    right.changed();
    anyhow::ensure!(
        take(&mut left_out)?.is_empty() && !take(&mut right_out)?.is_empty(),
        "skipping full change loses a forwarding progress message"
    );
    let origin = crate::origin::Origin { name: super::machine(0), key: left.store().book.directory.members[0].key };
    for on in [true, false] {
        crate::manage::change(&left, &origin, &super::account(0), Change::History { on })
            .0
            .map_err(anyhow::Error::msg)?;
        crate::manage_edit::forced_full(|| {
            crate::manage::change(&right, &origin, &super::account(0), Change::History { on })
        })
        .0
        .map_err(anyhow::Error::msg)?;
        let sent = take(&mut left_out)?;
        anyhow::ensure!(
            !sent.is_empty() && sent == take(&mut right_out)?,
            "forwarding guard preserves the full notification"
        );
    }
    println!(
        "forwarding_skip_counterexample=true machines=1 accounts=1 forwarding_guard_changes=2 notifications_equal=true"
    );
    Ok(())
}
