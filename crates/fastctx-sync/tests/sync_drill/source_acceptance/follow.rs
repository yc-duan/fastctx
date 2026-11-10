//! Two real authority scopes followed through JSON; a killed receiver resumes
//! its stream work even though the feed cursor already committed ahead of it.
use super::super::directory::Loopback;
use super::*;
use fastctx_sync::{
    authority::{Peer, Service},
    replicate::{DirectoryFollower, FollowProgress, ReplicaFollower},
    store::replica::{FollowStop, SqliteReplica},
    store::scopes::SqliteScopes,
};
use std::io::{BufRead, Write};

fn replica_device() -> DeviceId {
    DeviceId::from_bytes([4; 32])
}
fn node_b(dir: &Path) -> Result<(Database, SqliteBlocks, SqliteReplica, ReplicaFollower, DirectoryFollower)> {
    fastctx_platform::ensure_private_dir(dir)?;
    let db = Database::open_guarded(&dir.join("replica.sqlite3"), "", boundary::guard, schema::initialize)?;
    let blocks = SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        Holder::Device { device: replica_device() },
        Arc::new(Policy(account().id)),
        operation(220),
        1 << 24,
    )?;
    let replica = SqliteReplica::new(blocks.clone(), Arc::new(Verifier), identity());
    let follower = ReplicaFollower::new(replica.clone())
        .with_limits(1, 262144)?
        .with_kinds(vec![fastctx_sync::transfer::KindVersion { kind: KIND.into(), schema: 1 }])?;
    let directory = DirectoryFollower::new(
        SqliteScopes::new(blocks.clone(), HubId::from_bytes([99; 8]), replica_device()),
        identity(),
        Arc::new(Verifier),
    );
    Ok((db, blocks, replica, follower, directory))
}
async fn server(
    dir: &Path,
    seed: u8,
) -> Result<(Database, SqliteBlocks, SqliteAuthority, Arc<SqliteDirectory>, Arc<Service>)> {
    let (db, blocks) = open_hub(dir, false)?;
    let (run, authority) = authority(&blocks, seed, &Arc::new(Gate(AtomicBool::new(false)))).await?;
    let directory = Arc::new(SqliteDirectory::new(blocks.clone(), run, Arc::new(Signer(true))));
    let service = Arc::new(Service::new(directory.clone()).with_sources(Arc::new(authority.clone())));
    Ok((db, blocks, authority, directory, service))
}
fn loopback(service: Arc<Service>, budget: fastctx_sync::admission::ByteBudget) -> Loopback {
    Loopback { service, budget, peer: Peer { device: replica_device(), accounts: vec![account().id] } }
}
async fn append_to_hub(authority: &SqliteAuthority, object: &ObjectKey, after: u64) -> Result<()> {
    let sequence = Sequence(after + 1);
    let operation =
        OperationId::from_bytes(Digest::of(format!("follow/{}/{after}", object.object.as_str()).as_bytes()).bytes());
    let epoch = ProducerEpoch::from_bytes([42; 32]);
    let encoded = codec::encode(
        object.scope.clone(),
        format!("fact {} of {}", sequence.0, object.object.as_str()).into_bytes(),
        Encoding::Raw,
    )?;
    let batch = SourceBatch {
        format: 1,
        scope: object.scope.clone(),
        stream: StreamId::of(object, KIND),
        producer: Producer::Device { device: DeviceId::from_bytes([3; 32]) },
        epoch: epoch.clone(),
        operation: operation.clone(),
        kind: KIND.into(),
        schema: 1,
        after: Sequence(after),
        through: sequence,
        records: vec![SourceRecord {
            sequence,
            operation: operation.clone(),
            kind: KIND.into(),
            schema: 1,
            payload: encoded.representation.content.clone(),
            roots: Vec::new(),
            causes: Vec::new(),
        }],
    };
    authority
        .put_block(
            SourceUpload { context: context(object), holding: holding(object), kind: KIND.into(), epoch, operation },
            encoded.representation,
            encoded.bytes,
        )
        .await?;
    let accepted = authority.accept(context(object), holding(object), Signer(false).source(&batch)?).await?;
    assert!(matches!(accepted, SourceAccepted::Accepted { .. }));
    Ok(())
}
async fn discover(follower: &ReplicaFollower, directory: &DirectoryFollower, upstream: &Loopback) -> Result<()> {
    let mut cursor = None;
    loop {
        let page = follower.discover_page(directory, upstream, cursor).await?;
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(()),
        }
    }
}
async fn settle_follow(follower: &ReplicaFollower, upstream: &Loopback) -> Result<()> {
    for _ in 0..64 {
        let page = follower.poll(upstream, None, 64).await?;
        assert!(page.next.is_none());
        let mut complete = true;
        for (_, outcome) in page.scopes {
            if !matches!(outcome?, FollowProgress::CaughtUp | FollowProgress::Stopped(_)) {
                complete = false;
            }
        }
        if complete {
            return Ok(());
        }
    }
    Err("following did not settle within its bounded passes".into())
}
pub(crate) fn child(root: &Path) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(async {
        let (_, hub_blocks, _, _, service) = server(&root.join("hub"), 70).await?;
        let (db, _blocks, _replica, follower, directory) = node_b(&root.join("replica"))?;
        let upstream = loopback(service, hub_blocks.buffer_budget());
        discover(&follower, &directory, &upstream).await?;
        for _ in 0..32 {
            follower.step(&upstream, conversation(42).scope).await?;
            if count_scope(&db, "sync_stream_records", &conversation(42))? == 1 {
                println!("follow-committed-one");
                std::io::stdout().flush()?;
                loop {
                    std::thread::park();
                }
            }
        }
        Err("child did not reach its partial stream commit".into())
    })
}
fn kill_receiver(root: &Path) -> Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg(root)
        .arg("follow-child")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = send.send(result);
    });
    let result = receive.recv_timeout(Duration::from_secs(60));
    child.kill()?;
    assert!(!child.wait()?.success());
    assert!(
        matches!(result, Ok(Ok(line)) if line.trim()=="follow-committed-one"),
        "receiver did not reach the durable boundary"
    );
    Ok(())
}
fn reset_feed(path: &Path, scope: &ScopeKey) -> Result<()> {
    let mut db = fastctx_storage::rusqlite::Connection::open(path)?;
    let tx = db.transaction()?;
    let rows = tx
        .prepare("SELECT sequence,canonical_change FROM sync_changes WHERE hub=?1 AND scope=?2 ORDER BY sequence")?
        .query_map(params![scope.hub.to_string(), scope.scope.as_str()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let epoch = FeedEpoch::from_bytes([219; 32]);
    let mut prefix = fastctx_types::sync::feed::genesis(scope, &epoch);
    for (sequence, bytes) in rows {
        let change: fastctx_sync::catalog::CatalogChange = serde_json::from_slice(&bytes)?;
        prefix = fastctx_types::sync::feed::advance(&prefix, &change)?.1;
        tx.execute(
            "UPDATE sync_changes SET epoch=?3,prefix_digest=?4 WHERE hub=?1 AND scope=?2 AND sequence=?5",
            params![scope.hub.to_string(), scope.scope.as_str(), epoch.as_str(), prefix.as_str(), sequence],
        )?;
    }
    tx.execute(
        "UPDATE sync_scope_heads SET epoch=?3,prefix_digest=?4 WHERE hub=?1 AND scope=?2",
        params![scope.hub.to_string(), scope.scope.as_str(), epoch.as_str(), prefix.as_str()],
    )?;
    tx.commit()?;
    Ok(())
}
fn corrupt_cursor(path: &Path, scope: &ScopeKey) -> Result<()> {
    let db = fastctx_storage::rusqlite::Connection::open(path)?;
    let bytes: Vec<u8> = db.query_row(
        "SELECT cursor FROM sync_replica_scopes WHERE hub=?1 AND scope=?2",
        params![scope.hub.to_string(), scope.scope.as_str()],
        |row| row.get(0),
    )?;
    let mut cursor: fastctx_sync::cursor::ScopeCursor = serde_json::from_slice(&bytes)?;
    cursor.prefix_digest = Some(Digest::from_bytes([220; 32]));
    db.execute(
        "UPDATE sync_replica_scopes SET cursor=?3 WHERE hub=?1 AND scope=?2",
        params![scope.hub.to_string(), scope.scope.as_str(), serde_json::to_vec(&cursor)?],
    )?;
    Ok(())
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let root = root.join("follow");
    let hub_dir = root.join("hub");
    let replica_dir = root.join("replica");
    fastctx_platform::ensure_private_dir(&hub_dir)?;
    let (hub_db, hub_blocks, authority, directory, service) = server(&hub_dir, 1).await?;
    for number in [42, 43] {
        let object = conversation(number);
        describe(&directory, &object, number + 100).await?;
        for after in 0..3 {
            append_to_hub(&authority, &object, after).await?;
        }
    }
    drop((service, directory, authority, hub_blocks, hub_db));
    super::super::super::scheduler::closed(&hub_dir.join("hub.sqlite3")).await?;
    kill_receiver(&root)?;
    let (hub_db, hub_blocks, authority, directory, service) = server(&hub_dir, 100).await?;
    let upstream = loopback(service, hub_blocks.buffer_budget());
    let (db, blocks, replica, follower, discovered) = node_b(&replica_dir)?;
    let partial = replica.follow_scope(conversation(42).scope).await?.unwrap();
    assert_eq!(partial.cursor.through, Sequence(3));
    assert_eq!(count_scope(&db, "sync_stream_records", &conversation(42))?, 1);
    discover(&follower, &discovered, &upstream).await?;
    settle_follow(&follower, &upstream).await?;
    for n in [42, 43] {
        assert_eq!(count_scope(&db, "sync_stream_records", &conversation(n))?, 3);
        assert_eq!(prefixes(&db, &conversation(n))?, prefixes(&hub_db, &conversation(n))?);
    }
    let account_scope = ScopeKey { hub: hub(), scope: fastctx_sync::scope::account_scope(&account().id) };
    let skipped = replica.follow_scope(account_scope.clone()).await?.unwrap();
    assert!(skipped.view.kinds.is_empty());
    assert_eq!(skipped.cursor.through, Sequence(3));
    assert_eq!(
        db.read_now(|db| Ok(db.query_row(
            "SELECT COUNT(*) FROM sync_stream_records WHERE hub=?1 AND scope=?2",
            params![account_scope.hub.to_string(), account_scope.scope.as_str()],
            |row| row.get::<_, i64>(0)
        )?))?,
        0
    );
    let receipts = count_scope(&db, "sync_replica_head_receipts", &conversation(43))?;
    let changes = count_scope(&db, "sync_local_changes", &conversation(43))?;
    reset_feed(&hub_dir.join("hub.sqlite3"), &conversation(43).scope)?;
    assert!(matches!(follower.step(&upstream, conversation(43).scope).await?, FollowProgress::Reset));
    let reset = replica.follow_scope(conversation(43).scope).await?.unwrap();
    assert_eq!(reset.cursor.through, Sequence(0));
    assert_eq!(reset.generation, Revision(1));
    drop((discovered, follower, replica, blocks, db));
    super::super::super::scheduler::closed(&replica_dir.join("replica.sqlite3")).await?;
    let (db, _blocks, replica, follower, _directory) = node_b(&replica_dir)?;
    settle_follow(&follower, &upstream).await?;
    corrupt_cursor(&replica_dir.join("replica.sqlite3"), &conversation(43).scope)?;
    assert!(matches!(follower.step(&upstream, conversation(43).scope).await?, FollowProgress::Reset));
    settle_follow(&follower, &upstream).await?;
    assert_eq!(replica.follow_scope(conversation(43).scope).await?.unwrap().generation, Revision(2));
    assert_eq!(count_scope(&db, "sync_replica_head_receipts", &conversation(43))?, receipts);
    assert_eq!(count_scope(&db, "sync_local_changes", &conversation(43))?, changes);
    assert_eq!(count_scope(&db, "sync_stream_records", &conversation(43))?, 3);
    directory
        .publish(
            ScopeDescription {
                scope: conversation(42).scope,
                owner: account(),
                subject: ScopeSubject::Conversation {
                    conversation: conversation(42).object,
                    shared_on: None,
                    born_seal: None,
                },
                visibility: ScopeVisibility::Current,
                acl_revision: Revision(2),
                lifecycle: Lifecycle::Deleted,
            },
            operation(230),
        )
        .await?;
    assert!(matches!(
        follower.step(&upstream, conversation(42).scope).await?,
        FollowProgress::Stopped(FollowStop::Revoked)
    ));
    append_to_hub(&authority, &conversation(43), 3).await?;
    settle_follow(&follower, &upstream).await?;
    assert_eq!(count_scope(&db, "sync_stream_records", &conversation(43))?, 4);
    assert_eq!(count_scope(&db, "sync_stream_records", &conversation(42))?, 3);
    assert!(matches!(replica.follow_scope(conversation(42).scope).await?.unwrap().stopped, Some(FollowStop::Revoked)));
    drop((upstream, directory, authority, hub_blocks, hub_db));
    println!(
        "replica following: two scopes, JSON evidence, killed receiver continuation, feed reset across restart, same-epoch reset without reinstall and isolated revocation passed"
    );
    Ok(())
}
