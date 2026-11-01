//! A run answer becomes heard only when its host confirms that answer. All
//! authenticated aliases and retirement of the answer commit in one transaction.

use crate::{Node, records};
use fastctx_types::run::{JobDelivery, RunReply, RunState};
use fastctx_types::{Address, JobId, MachineName, ResidentId};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Serialize, Deserialize)]
struct Receipt {
    output: String,
    ids: Vec<fastctx_types::NoticeId>,
}

fn key(machine: &MachineName, job: &JobId) -> String {
    crate::speech::fixed_id("run", &[machine.key(), job.as_str()]).to_string()
}
pub(crate) fn prepare(node: &Node, resident: &ResidentId) -> io::Result<fastctx_types::layout::ConversationDir> {
    let directory = node.world.conversation(resident);
    let folder = directory.book("run-receipts");
    records::ensure_each(&directory, "run-receipts", move |push| {
        for path in crate::retention::entries(&folder)? {
            if path.extension().is_none_or(|extension| extension != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let row: Receipt = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
                let key = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| io::Error::other("invalid run receipt"))?
                    .to_string();
                push(records::Source::new(key, path.clone(), serde_json::to_vec(&row)?)?)
            })?;
        }
        Ok(())
    })?;
    crate::inbox::storage::initialize(&directory)?;
    Ok(directory)
}
pub(crate) fn remember(node: &Node, owner: &Address, reply: &RunReply) -> io::Result<()> {
    if !node.is_me(&owner.machine) || matches!(reply.state, RunState::Running | RunState::Exited { .. }) {
        return Ok(());
    }
    let id = crate::inbox::end_id(owner, &reply.machine, &reply.job, None);
    let mut ids = vec![id.clone()];
    if let Ok(identity) = crate::letters::Identity::at(node, &(&reply.machine).into(), id.as_str()) {
        ids.extend(identity.ids().cloned());
    }
    let directory = prepare(node, &owner.resident)?;
    let (key, receipt) = (key(&reply.machine, &reply.job), Receipt { output: reply.output.log.clone(), ids });
    let bytes = serde_json::to_vec(&receipt)?.len();
    records::write_at(&directory, fastctx_storage::Priority::Normal, bytes, move |rows| {
        rows.put_until(
            "run-receipts",
            &key,
            &receipt,
            Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24)),
        )
    })
}
pub(crate) fn confirmed(node: &Node, owner: &Address, machine: &MachineName, delivery: &JobDelivery) -> io::Result<()> {
    if !node.is_me(&owner.machine) {
        return Ok(());
    }
    let directory = prepare(node, &owner.resident)?;
    let (key, output) = (key(machine, &delivery.job), delivery.output.clone());
    records::write_at(&directory, fastctx_storage::Priority::Interactive, 512, move |rows| {
        let Some(receipt) = rows.get::<Receipt>("run-receipts", &key)? else { return Ok(()) };
        if receipt.output != output {
            return Err(fastctx_storage::Error::new("the run answer receipt refers to another output"));
        }
        for id in receipt.ids {
            if !crate::inbox::storage::delivered(rows, id.as_str())? {
                rows.put("delivery-receipts", id.as_str(), &crate::inbox::storage::Receipt::Answered)?;
            }
        }
        rows.delete("run-receipts", &key)?;
        Ok(())
    })
}
