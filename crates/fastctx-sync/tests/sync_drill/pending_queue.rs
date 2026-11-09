//! Ordinary concurrent editors wait for semantic capacity without losing edits.
use super::*;

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("pending-queue");
    fastctx_platform::ensure_private_dir(&dir)?;
    let db = open(&dir.join("replica.sqlite3"))?;
    let gate = policy(false);
    let blocks = Arc::new(SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        holder(),
        gate.clone(),
        operation(236),
        8 * 1024 * 1024,
    )?);
    let store = SqliteBackend::new(Config {
        database: db.clone(),
        hub: scope().hub,
        holder: holder(),
        policy: gate,
        blocks: blocks.clone(),
        handlers: vec![Arc::new(TextHandler)],
        authorizers: vec![Arc::new(Permit)],
        semantic_bytes: 1024 * 1024,
    })?;
    let content = codec::encode(scope(), vec![b'x'; 1024 * 1024], Encoding::Raw)?;
    let mut requests = Vec::new();
    for n in 80..88 {
        let held = blocks
            .put(custody(n, "conversation.plan"), operation(n), content.representation.clone(), content.bytes.clone())
            .await?;
        requests.push(request(n, vec![(n, ContentRef::from(held.content))]));
    }
    let mut waiting = tokio::task::JoinSet::new();
    for submission in requests {
        let store = store.clone();
        waiting.spawn(async move { store.submit_pending(principal(), AtomicMutation::new(submission)?).await });
    }
    let mut sequences = std::collections::BTreeSet::new();
    while let Some(result) = waiting.join_next().await {
        sequences.insert(result??.local.0);
    }
    assert_eq!(sequences.len(), 8);
    assert_eq!(count(&db, "sync_pending_mutations")?, 8);
    assert_eq!(count(&db, "sync_outbox")?, 8);
    Ok(())
}
