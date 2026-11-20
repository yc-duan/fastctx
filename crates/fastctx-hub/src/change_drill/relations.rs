//! Relation changes (`manage_edit::relation`) made with their narrow edit and
//! `Hub::relations_changed` beside the same changes made in full: the same
//! answer, the same sessions ended, the same calls and streams cut, the same
//! runtime state after, and a reopened book equal to what was committed. A
//! list the change touched but its narrow edit left unsaved shows up as a
//! reopened book that differs.

use std::path::Path;

use anyhow::Context;
use fastctx_net::wire::Change;
use serde_json::Value;

use super::equivalence::{copy_dir, effects, wait_closed};

/// Each kind is made twice: a share and an assistance taken away and given
/// back, a plan set and taken away, a project shared and unshared.
pub(super) const KINDS: [&str; 5] = ["Share", "Assist", "Plan", "StopActing", "ShareProject"];

fn request(kind: &str, round: usize, accounts: usize) -> Change {
    let again = round == 1;
    match kind {
        // m0 is a0's and shared to a1, which m0 acts for: taking it away
        // also settles that acting entry.
        "Share" => Change::Share { machine: super::machine(0), to: super::account(1), shared: again, jobs: None },
        "Assist" => Change::Assist {
            machine: super::machine(accounts),
            to: super::account(1),
            agent: "codex".parse().expect("agent"),
            home: None,
            assisting: again,
            until: None,
            weekly: None,
            daily: None,
            five_hour: None,
            turns: None,
            share: None,
            manages: None,
        },
        "Plan" => Change::Plan {
            machine: super::machine(0),
            agent: "codex".parse().expect("agent"),
            home: None,
            plan: (!again).then_some(fastctx_types::accounts::PlanLabel::Max),
            ultra: false,
        },
        "StopActing" => Change::StopActing { machine: super::machine(0) },
        "ShareProject" => Change::ShareProject {
            town: fastctx_types::access::TownId::from_bytes([7; 8]),
            with: super::account(1),
            shared: !again,
        },
        _ => unreachable!(),
    }
}

pub(super) fn exercise(root: &Path, base: &Path, seed: u64, kind: &str, round: usize) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let entered = runtime.enter();
    let left_dir = root.join(format!("{seed}-{kind}-relation-narrow"));
    let right_dir = root.join(format!("{seed}-{kind}-relation-full"));
    if round == 0 {
        copy_dir(base, &left_dir)?;
        copy_dir(base, &right_dir)?;
    }
    let left = super::local_hub::open(&left_dir)?;
    let right = super::local_hub::open(&right_dir)?;
    let mut receivers = Vec::new();
    for hub in [&left, &right] {
        let standings = crate::control::change_probe::seed(hub);
        receivers.push(crate::data::change_probe::seed(hub, &standings));
        crate::updates::change_probe::seed(hub);
        crate::updates::change_probe::flush(hub);
        crate::afoot::change_probe::seed(hub);
        hub.changed();
    }
    anyhow::ensure!(effects(&left)? == effects(&right)?, "same initial runtime state");
    let accounts = left.store().book.directory.accounts.len();
    let origin = crate::origin::Origin { name: super::machine(0), key: left.store().book.directory.members[0].key };
    let change = request(kind, round, accounts);
    anyhow::ensure!(crate::manage_edit::relation(&change), "a relation change");
    let (answer_left, ended_left) = crate::manage::change(&left, &origin, &super::account(0), change.clone());
    let (answer_right, ended_right) =
        crate::manage_edit::forced_full(|| crate::manage::change(&right, &origin, &super::account(0), change));
    let told = |answer: Result<fastctx_net::wire::Told, String>| -> anyhow::Result<Value> {
        Ok(match answer {
            Ok(told) => serde_json::to_value(told)?,
            Err(reason) => Value::String(reason),
        })
    };
    let (answer_left, answer_right) = (told(answer_left)?, told(answer_right)?);
    anyhow::ensure!(answer_left == answer_right, "same answer: {answer_left} and {answer_right}");
    let ended = |ended: &[(fastctx_types::MachineName, fastctx_net::wire::Refusal)]| {
        ended.iter().map(|(name, refusal)| format!("{name} {refusal:?}")).collect::<Vec<_>>()
    };
    anyhow::ensure!(ended(&ended_left) == ended(&ended_right), "same sessions ended");
    let (after_left, after_right) = (effects(&left)?, effects(&right)?);
    anyhow::ensure!(after_left == after_right, "same runtime state after: {after_left} and {after_right}");
    let cut = |receivers: &mut Vec<tokio::sync::oneshot::Receiver<String>>| {
        receivers.iter_mut().map(|receiver| receiver.try_recv().is_ok()).collect::<Vec<_>>()
    };
    let (cut_left, cut_right) = {
        let (left_receivers, right_receivers) = receivers.split_at_mut(1);
        (cut(&mut left_receivers[0]), cut(&mut right_receivers[0]))
    };
    anyhow::ensure!(cut_left == cut_right, "the same live streams cut");
    // Each hub gives an assistance a grant of its own at random: the books
    // are the same but for those.
    let without_grants = |mut book: Value| {
        if let Some(assists) = book.get_mut("lends").and_then(Value::as_array_mut) {
            for assist in assists.iter_mut().filter_map(Value::as_object_mut) {
                assist.remove("grant");
            }
        }
        book
    };
    let book_left = serde_json::to_value(&*left.store().book)?;
    let book_right = serde_json::to_value(&*right.store().book)?;
    anyhow::ensure!(
        without_grants(book_left.clone()) == without_grants(book_right.clone()),
        "books equal after round {round}"
    );
    anyhow::ensure!(
        left.store().indexed().all.shares.len() == left.store().book.directory.shares.len(),
        "the index rebuilt with the book"
    );
    drop((left, right));
    drop(entered);
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    wait_closed(&left_dir)?;
    wait_closed(&right_dir)?;
    let reopened_left = crate::Store::open(&left_dir).context("reopening the narrow hub")?;
    let reopened_right = crate::Store::open(&right_dir).context("reopening the full hub")?;
    let reopened = serde_json::to_value(&*reopened_left.book)?;
    let reopened_other = serde_json::to_value(&*reopened_right.book)?;
    anyhow::ensure!(reopened == book_left, "the narrow hub's reopened book equals what it committed");
    anyhow::ensure!(reopened_other == book_right, "the full hub's reopened book equals what it committed");
    anyhow::ensure!(
        without_grants(reopened) == without_grants(reopened_other),
        "both reopened books equal but for their grants"
    );
    drop((reopened_left, reopened_right));
    wait_closed(&left_dir)?;
    wait_closed(&right_dir)?;
    Ok(())
}
