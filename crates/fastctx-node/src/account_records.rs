//! Local subscriptions to authoritative account record metadata.

use crate::{Node, town::caller};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::{
    AccountName, HubName,
    ipc::{Failure, NodeFrame, Peer},
};
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn watch(
    node: &Arc<Node>,
    peer: &Peer,
    hub: Option<HubName>,
    account: Option<AccountName>,
    kind: String,
    owner: Option<fastctx_types::ResidentId>,
    shapes: u32,
    id: u64,
    mut reader: FrameReader,
    mut writer: FrameWriter,
) -> std::io::Result<()> {
    let opening = (|| {
        let (link, account) = crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
        let watch = link.watch_account_records(account.clone(), kind).map_err(Failure::new)?.for_plan(owner);
        Ok::<_, Failure>((link, account, watch))
    })();
    let failure = match opening {
        Err(failure) => failure,
        Ok((link, account, mut watch)) => loop {
            let event = tokio::select! {
                event = watch.next() => event.map_err(Failure::new),
                _ = reader.recv() => return Ok(()),
            };
            let event = match event {
                Ok(event) => event,
                Err(failure) => break failure,
            };
            if let Err(failure) = caller::of(node, peer)
                .and_then(|caller| crate::profile::scope(node, &caller, Some(&link.hub_name()), Some(&account)))
            {
                break failure;
            }
            if shapes < 2
                && match &event {
                    fastctx_types::account_records::Watched::Current { entries } => {
                        entries.iter().any(|metadata| metadata.by.is_none())
                    }
                    fastctx_types::account_records::Watched::Changed { metadata } => metadata.by.is_none(),
                }
            {
                break Failure::not_run("update this client to display shared record attribution");
            }
            writer.send(&serde_json::to_vec(&NodeFrame::AccountRecords { id, event })?).await?;
        },
    };
    writer.send(&serde_json::to_vec(&NodeFrame::Reply { id, reply: Err(failure) })?).await?;
    writer.drain().await
}
