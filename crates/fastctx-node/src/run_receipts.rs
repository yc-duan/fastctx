//! A run answer becomes heard only when its host confirms that answer.
//! Remember the authenticated completion identities before returning it;
//! receiving an answer at the node alone must not suppress recovery mail.

use std::io;
use std::path::PathBuf;

use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::run::{JobDelivery, RunReply, RunState};
use fastctx_types::{Address, JobId, MachineName, NoticeId, ResidentId};
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Serialize, Deserialize)]
struct Receipt {
    output: String,
    ids: Vec<NoticeId>,
}

fn path(node: &Node, resident: &ResidentId, machine: &MachineName, job: &JobId) -> PathBuf {
    let id = crate::speech::fixed_id("run", &[machine.key(), job.as_str()]);
    node.world.conversation(resident).book("run-receipts").join(format!("{id}.json"))
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
    let path = path(node, &owner.resident, &reply.machine, &reply.job);
    ensure_private_dir(path.parent().expect("run receipt directory"))?;
    let receipt = Receipt { output: reply.output.log.clone(), ids };
    fastctx_install::renewal::write_json(&path, &receipt)
}

/// The caller may already hold the records lock. Atomic receipt publication
/// also suppresses an inbox entry concurrently kept under its original id.
pub(crate) fn confirmed(node: &Node, owner: &Address, machine: &MachineName, delivery: &JobDelivery) -> io::Result<()> {
    if !node.is_me(&owner.machine) {
        return Ok(());
    }
    let path = path(node, &owner.resident, machine, &delivery.job);
    let Some(receipt) = fastctx_install::renewal::read_json::<Receipt>(&path)? else { return Ok(()) };
    if receipt.output != delivery.output {
        return Err(io::Error::other("the run answer receipt refers to another output"));
    }
    let directory = node.world.conversation(&owner.resident).delivery_receipts();
    ensure_private_dir(&directory)?;
    for id in receipt.ids {
        match publish_new(&directory.join(id.as_str()), b"answered", Permissions::Private) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            result => result?,
        }
    }
    std::fs::remove_file(path)
}
