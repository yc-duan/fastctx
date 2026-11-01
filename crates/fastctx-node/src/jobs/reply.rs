//! Complete the durable result after execution without making the coordinating
//! task wait synchronously for a cold record family or a large projection.
use crate::Node;
use fastctx_types::{
    ipc::Failure,
    run::{JobRecord, RunReply, RunState},
};
use std::{path::PathBuf, sync::Arc};

pub(super) async fn finish(
    node: &Arc<Node>,
    path: PathBuf,
    record: JobRecord,
    mut reply: RunReply,
) -> Result<(RunReply, JobRecord), Failure> {
    let node = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        reply.scratch = crate::scratch::jobs::locations(&path, &reply.machine)
            .map_err(|error| super::failure("reading scratch locations", error))?;
        if record.call_id.is_some() {
            let mut facts = crate::run_facts::assemble(&reply, &path)
                .map_err(|error| super::failure("recording run facts", error))?;
            if matches!(reply.state, RunState::Ended { .. }) && !facts.output.as_ref().is_some_and(|output| output.cut)
            {
                facts.output = None;
            }
            reply.run = Some(Box::new(facts));
        }
        if let Some(owner) = record.owner.resident() {
            crate::run_receipts::remember(&node, owner, &reply)
                .map_err(|error| super::failure("remembering the run answer", error))?;
        }
        Ok((reply, record))
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
}
