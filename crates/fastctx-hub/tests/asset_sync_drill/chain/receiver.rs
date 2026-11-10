use super::*;
use fastctx_sync::{
    blocks::codec,
    manifest::*,
    replicate::*,
    store::{
        Error,
        replica::{ReplicaContext, SqliteReplica},
    },
    transfer::{KindVersion, Message},
};
use futures_util::StreamExt;

async fn content(
    replica: &SqliteReplica,
    upstream: &dyn Upstream,
    context: &ReplicaContext,
    c: &Config,
    phase: &str,
    root: &Path,
    mut counts: retry::Counts,
) -> anyhow::Result<()> {
    let mut after = None;
    let began = std::time::Instant::now();
    for _ in 0..8192 {
        let missing = replica.missing_content(context.clone(), c.object(), KIND.into(), after, 16).await?;
        if !missing.pending {
            let data = data::load(root)?;
            let seconds = began.elapsed().as_secs_f64();
            std::fs::write(
                root.join(format!("result-{phase}.json")),
                serde_json::to_vec(&serde_json::json!({
                "bytes":data.bytes, "seconds":seconds, "mib_per_second":data.bytes as f64/1048576.0/seconds, "busy":counts.busy, "unavailable":counts.unavailable }))?,
            )?;
            return Ok(());
        }
        let outcomes = futures_util::stream::iter(missing.requests.into_iter().map(|request| async move {
            let mut counts = retry::Counts::default();
            let result = retry::request(&mut counts, || async {
                match request.root().clone() {
                    ContentRef::Single { block, decoded_bytes } => {
                        let received = upstream
                            .fetch(BlockRequest {
                                holding: request.holding().clone(),
                                object: request.object().clone(),
                                kind: KindVersion { kind: KIND.into(), schema: 1 },
                                root: request.root().clone(),
                                block: ContentBlock { id: block, decoded_bytes },
                            })
                            .await?;
                        let encoded = codec::encode(c.object().scope, received.value().clone(), Encoding::Raw)?;
                        replica.receive_block(request.clone(), encoded.representation, encoded.bytes).await
                    }
                    ContentRef::Manifest { root } => {
                        let answer = upstream
                            .exchange(Message::GetManifest { holding: request.holding().clone(), root })
                            .await?;
                        let Message::Manifest { reply, .. } = answer.value() else {
                            return Err(Error::Invalid("expected manifest".into()));
                        };
                        replica.receive_manifest(request.clone(), reply.clone()).await
                    }
                }
            })
            .await;
            (result, counts)
        }))
        .buffer_unordered(3)
        .collect::<Vec<_>>()
        .await;
        for (result, seen) in outcomes {
            counts.busy += seen.busy;
            counts.unavailable += seen.unavailable;
            result?;
        }
        after = missing.next;
    }
    anyhow::bail!("body closure did not complete")
}
pub(super) async fn run(root: &Path, n: usize, pause: bool, phase: &str, revoke: bool) -> anyhow::Result<()> {
    let c = Config::load(root)?;
    let dir = root.join(format!("node-{n}-{phase}"));
    let (db, blocks) = node::blocks(&dir, &c, n)?;
    let (link, upstream) = node::connect(&dir, &c, n, &blocks).await?;
    let replica = SqliteReplica::new(blocks.clone(), c.verifier(), c.identity.clone());
    let follower = ReplicaFollower::new(replica.clone())
        .with_limits(1, 262144)?
        .with_kinds(vec![KindVersion { kind: KIND.into(), schema: 1 }])?;
    let directory = node::directory(&blocks, &c, n);
    let mut counts = retry::Counts::default();
    let mut cursor = None;
    loop {
        let page =
            retry::request(&mut counts, || follower.discover_page(&directory, upstream.as_ref(), cursor.clone()))
                .await?;
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let mut complete = false;
    for _ in 0..128 {
        let progress = retry::request(&mut counts, || follower.step(upstream.as_ref(), c.conversation().scope)).await?;
        let count = db.read_now(|db| {
            Ok(db.query_row("SELECT count(*) FROM sync_stream_records WHERE kind=?1", [KIND], |row| {
                row.get::<_, i64>(0)
            })?)
        })?;
        if pause && count == 1 {
            std::fs::write(root.join("pause-b"), b"one source record committed")?;
            std::future::pending::<()>().await;
        }
        if matches!(progress, FollowProgress::CaughtUp) {
            anyhow::ensure!(count == 4, "replica reported caught up with missing records");
            complete = true;
            break;
        }
    }
    anyhow::ensure!(complete, "replica never caught up");
    let state = replica.follow_scope(c.conversation().scope).await?.unwrap();
    let context = ReplicaContext {
        source: fastctx_sync::identity::Producer::Hub,
        holding: state.holding,
        source_uses: Default::default(),
    };
    let name = format!("{n}-{phase}");
    content(&replica, upstream.as_ref(), &context, &c, &name, root, counts).await?;
    let data = data::load(root)?;
    db.read_now(|db| {
        let mut query = db.prepare(
            "SELECT EXISTS(SELECT 1 FROM sync_asset_blocks
            WHERE hub=?1 AND scope=?2 AND hash=?3 AND verified=1)",
        )?;
        for block in &data.blocks {
            let present: bool = query.query_row(
                fastctx_storage::rusqlite::params![
                    block.id.domain.hub.to_string(),
                    block.id.domain.scope.as_str(),
                    block.id.digest.as_str()
                ],
                |r| r.get(0),
            )?;
            if !present {
                return Err(fastctx_storage::Error::with_kind(
                    fastctx_storage::ErrorKind::Invalid,
                    "expected body block is not stored",
                ));
            }
        }
        Ok(())
    })?;
    let sample = data.blocks[0].clone();
    let request = BlockRequest {
        holding: context.holding.clone(),
        object: c.object(),
        kind: KindVersion { kind: KIND.into(), schema: 1 },
        root: ContentRef::from(sample.clone()),
        block: sample,
    };
    if revoke {
        anyhow::ensure!(upstream.fetch(request.clone()).await?.len() == fastctx_types::sync::BLOCK_BYTES as usize);
        let told = link.manage(&c.account.name, fastctx_net::wire::Change::Logout).await.map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            matches!(told, fastctx_net::wire::Told::Directory { .. } | fastctx_net::wire::Told::Done),
            "logout failed: {told:?}"
        );
        anyhow::ensure!(
            matches!(upstream.fetch(request).await, Err(Error::Denied(_) | Error::Revoked)),
            "revoked replica could still fetch content"
        );
    }
    drop(upstream);
    link.stop();
    Ok(())
}
