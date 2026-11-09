//! Upkeep beside the task worker: steps a stopped process left open are
//! reconciled once at start; afterwards finished steps release their staged
//! pins and collection reclaims what nothing retains, a page at a time.
use std::{sync::Arc, time::Duration};

use fastctx_sync::{blocks::SqliteBlocks, manifest::BlockStore};

/// Steps, pins or contents one call handles.
const PAGE: u32 = 64;
/// Pages of collection one round takes before yielding to the next round.
const COLLECT_PAGES: usize = 16;
const ROUND: Duration = Duration::from_secs(30);

pub(super) async fn run(blocks: SqliteBlocks, log: Arc<crate::log::Log>) {
    let mut cursor = None;
    loop {
        match blocks.reconcile_steps(cursor.clone(), PAGE).await {
            Ok(Some(next)) => cursor = Some(next),
            Ok(None) => break,
            Err(error) => {
                log.line(format_args!("reconciling interrupted source steps: {error}"));
                break;
            }
        }
    }
    loop {
        if let Err(error) = round(&blocks).await {
            log.line(format_args!("sync upkeep: {error}"));
        }
        tokio::time::sleep(ROUND).await;
    }
}

async fn round(blocks: &SqliteBlocks) -> Result<(), fastctx_sync::store::Error> {
    while blocks.drain_steps(PAGE).await? != 0 {}
    for _ in 0..COLLECT_PAGES {
        if blocks.collect(PAGE).await? == 0 {
            break;
        }
    }
    Ok(())
}
