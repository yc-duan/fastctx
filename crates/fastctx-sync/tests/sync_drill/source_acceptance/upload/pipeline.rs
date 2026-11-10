//! One stream exercises overlapping offers, overlapping proofs and prefix ordering.
use super::*;

pub(super) async fn run(root: &Path, blocks_per_batch: u8, latency: Duration) -> Result<()> {
    let root = root.join("source-pipeline");
    let hub_dir = root.join("hub");
    let node_dir = root.join("node");
    fastctx_platform::ensure_private_dir(&hub_dir)?;
    fastctx_platform::ensure_private_dir(&node_dir)?;
    let (hub_db, hub_blocks, authority, directory, service) = server(&hub_dir, 1).await?;
    let (db, blocks, sources, target) = node(&node_dir, &conversation(42))?;
    data::bulk(&sources, &target, blocks_per_batch).await?;
    assert_eq!(count_scope(&db, "sync_source_proofs", &conversation(42))?, 4);
    promote(&blocks, &directory).await?;
    let uploader = SourceUploader::new(sources.clone(), hub())?;
    let mut transport = UploadLoopback::new(service, hub_blocks.buffer_budget(), false);
    transport.latency = latency;
    transport.audit_staging.store(true, Ordering::Release);
    transport.waiting_once.store(!latency.is_zero(), Ordering::Release);
    transport.partial_once.store(!latency.is_zero(), Ordering::Release);
    let upstream = Arc::new(transport);
    let started = std::time::Instant::now();
    for iteration in 0..128 {
        match uploader.step(upstream.clone(), key()).await {
            Ok(SourceUploadProgress::Idle(state)) if state.through == Sequence(4) => break,
            Ok(SourceUploadProgress::Stopped(state)) => {
                return Err(format!("pipeline stopped: {:?}", state.stopped).into());
            }
            Err(Error::Busy) => {}
            result => {
                result?;
            }
        }
        assert!(iteration < 127, "pipeline did not settle");
    }
    let elapsed = started.elapsed();
    let bytes = upstream.bytes.load(Ordering::Acquire);
    assert!(bytes >= usize::from(blocks_per_batch) * 4 * fastctx_types::sync::BLOCK_BYTES as usize);
    assert!(upstream.offer_max.load(Ordering::Acquire) > 1, "offers waited for each individual round trip");
    assert!(
        upstream.source_max.load(Ordering::Acquire) > 1,
        "Source submissions waited for each preceding acknowledgement"
    );
    assert_eq!(count_scope(&hub_db, "sync_stream_records", &conversation(42))?, 4);
    assert_eq!(count_scope(&hub_db, "sync_source_proofs", &conversation(42))?, 4);
    settle(&db, &blocks, Some(&sources), None).await?;
    settle(&hub_db, &hub_blocks, None, Some(&authority)).await?;
    assert_eq!(prefixes(&db, &conversation(42))?, prefixes(&hub_db, &conversation(42))?);
    assert_eq!(blocks.buffer_budget().reserved(), 0);
    assert_eq!(hub_blocks.buffer_budget().reserved(), 0);
    println!(
        "source pipeline: {} bytes in {:.3}s = {:.2} MiB/s; peak offers {}, Source {}; staging isolation and contiguous prefixes passed",
        bytes,
        elapsed.as_secs_f64(),
        bytes as f64 / elapsed.as_secs_f64() / 1048576.0,
        upstream.offer_max.load(Ordering::Acquire),
        upstream.source_max.load(Ordering::Acquire)
    );
    Ok(())
}
