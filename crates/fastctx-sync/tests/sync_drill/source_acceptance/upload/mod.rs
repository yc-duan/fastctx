//! A killed producer resumes exact split proofs; promotion and tenure fence every IO.
use super::super::directory::Loopback;
use super::*;
use fastctx_sync::transfer::Message;
use fastctx_sync::{
    authority::{Peer, Service},
    replicate::{SourceUploadKey, SourceUploadPhase, SourceUploadProgress, SourceUploadStop, SourceUploader},
    store::scopes::{ScopeState, SqliteScopes},
};
use std::io::{BufRead, Write};
mod data;
mod pipeline;
mod transport;
use transport::UploadLoopback;

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
fn key() -> SourceUploadKey {
    let object = conversation(42);
    SourceUploadKey {
        scope: object.scope,
        stream: StreamId::of(&timeline(&conversation(42)), KIND),
        epoch: ProducerEpoch::from_bytes([42; 32]),
    }
}
async fn promote(blocks: &SqliteBlocks, directory: &SqliteDirectory) -> Result<()> {
    let object = conversation(42);
    let proof = directory
        .publish(
            ScopeDescription {
                scope: object.scope.clone(),
                owner: account(),
                subject: ScopeSubject::Conversation { conversation: object.object, shared_on: None, born_seal: None },
                visibility: ScopeVisibility::Current,
                acl_revision: Revision(1),
                lifecycle: Lifecycle::Live,
            },
            operation(214),
        )
        .await?;
    let verified = fastctx_sync::scope::verify(&object.scope, proof, &identity(), &Verifier)?;
    SqliteScopes::new(blocks.clone(), HubId::from_bytes([99; 8]), DeviceId::from_bytes([3; 32]))
        .install(&verified)
        .await?;
    Ok(())
}
async fn drive(uploader: &SourceUploader, upstream: &Arc<UploadLoopback>) -> Result<()> {
    for _ in 0..128 {
        match uploader.step(upstream.clone(), key()).await? {
            SourceUploadProgress::Idle(state) if state.through == Sequence(2) => return Ok(()),
            SourceUploadProgress::Stopped(state) => return Err(format!("upload stopped: {:?}", state.stopped).into()),
            _ => {}
        }
    }
    Err("source upload did not settle".into())
}
pub(crate) fn child(root: &Path) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(async {
        let (_, hub_blocks, _, _, service) = server(&root.join("hub"), 70).await?;
        let (_, _, sources, _) = node(&root.join("node"), &conversation(42))?;
        let uploader = SourceUploader::new(sources, hub())?;
        drive(&uploader, &Arc::new(UploadLoopback::new(service, hub_blocks.buffer_budget(), true))).await
    })
}
fn kill(root: &Path) -> Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg(root)
        .arg("upload-child")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let value = std::io::BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = send.send(value);
    });
    let reached = receive.recv_timeout(Duration::from_secs(60));
    child.kill()?;
    assert!(!child.wait()?.success());
    assert!(
        matches!(reached,Ok(Ok(line)) if line.trim()=="source-upload-hub-accepted"),
        "upload child missed the accepted-but-unacknowledged boundary"
    );
    Ok(())
}
pub(crate) async fn benchmark(root: &Path) -> Result<()> {
    Box::pin(pipeline::run(root, 16, Duration::ZERO)).await
}
pub(crate) async fn run(root: &Path) -> Result<()> {
    let root = root.join("source-upload");
    let hub_dir = root.join("hub");
    let node_dir = root.join("node");
    fastctx_platform::ensure_private_dir(&hub_dir)?;
    fastctx_platform::ensure_private_dir(&node_dir)?;
    let (hub_db, hub_blocks, authority, directory, service) = server(&hub_dir, 1).await?;
    let (db, blocks, sources, target) = node(&node_dir, &conversation(42))?;
    assert_eq!(Box::pin(data::ingest(&sources, &target)).await?, key());
    assert_eq!(count_scope(&db, "sync_source_proofs", &conversation(42))?, 2);
    let uploader = SourceUploader::new(sources.clone(), hub())?;
    let upstream = Arc::new(UploadLoopback::new(service, hub_blocks.buffer_budget(), false));

    assert!(matches!(uploader.step(upstream.clone(), key()).await?, SourceUploadProgress::NotPromoted { .. }));
    assert_eq!(upstream.calls.load(Ordering::Acquire), 0);
    promote(&blocks, &directory).await?;
    drop((upstream, uploader, target, sources, blocks, db, authority, directory, hub_blocks, hub_db));
    super::super::super::scheduler::closed(&node_dir.join("node.sqlite3")).await?;
    super::super::super::scheduler::closed(&hub_dir.join("hub.sqlite3")).await?;
    kill(&root)?;
    let (hub_db, hub_blocks, authority, _directory, service) = server(&hub_dir, 100).await?;
    let (db, blocks, sources, _) = node(&node_dir, &conversation(42))?;
    let uploader = SourceUploader::new(sources.clone(), hub())?;
    let interrupted = uploader.status(key()).await?.unwrap();
    assert_eq!(interrupted.through, Sequence(0));
    assert_eq!(interrupted.phase, SourceUploadPhase::Submit);
    assert!((1..=2).contains(&count_scope(&hub_db, "sync_stream_records", &conversation(42))?));
    let first_operation = db.read_now(|db| {
        Ok(db.query_row(
            "SELECT operation FROM sync_source_proofs WHERE scope=?1 ORDER BY first_sequence LIMIT 1",
            params![key().scope.scope.as_str()],
            |r| r.get::<_, String>(0),
        )?)
    })?;
    let upstream = Arc::new(UploadLoopback::new(service, hub_blocks.buffer_budget(), false));
    drive(&uploader, &upstream).await?;
    assert!(upstream.offers.lock().unwrap().iter().all(|op| op.as_str() != first_operation));
    assert_eq!(count_scope(&hub_db, "sync_stream_records", &conversation(42))?, 2);
    assert_eq!(count_scope(&hub_db, "sync_source_proofs", &conversation(42))?, 2);
    assert_eq!(uploader.status(key()).await?.unwrap().through, Sequence(2));
    settle(&db, &blocks, Some(&sources), None).await?;
    settle(&hub_db, &hub_blocks, None, Some(&authority)).await?;
    assert_eq!(prefixes(&db, &conversation(42))?, prefixes(&hub_db, &conversation(42))?);
    let calls = upstream.calls.load(Ordering::Acquire);
    assert!(matches!(uploader.step(upstream.clone(), key()).await?, SourceUploadProgress::Idle(_)));
    assert_eq!(calls, upstream.calls.load(Ordering::Acquire));
    let scopes = SqliteScopes::new(blocks.clone(), HubId::from_bytes([99; 8]), DeviceId::from_bytes([3; 32]));
    scopes
        .advance(
            key().scope,
            ScopeState { acl_revision: Revision(1), publication_epoch: Revision(1), enabled: true },
            ScopeState { acl_revision: Revision(1), publication_epoch: Revision(2), enabled: true },
        )
        .await?;
    assert!(
        matches!(uploader.step(upstream.clone(),key()).await?,SourceUploadProgress::Stopped(state) if state.stopped==Some(SourceUploadStop::HoldingChanged))
    );
    assert_eq!(calls, upstream.calls.load(Ordering::Acquire));
    let restarted = SourceUploader::new(sources, hub())?;
    assert!(matches!(restarted.step(upstream.clone(), key()).await?, SourceUploadProgress::Stopped(_)));
    println!(
        "source upload: unpromoted silence, canonical split proofs, admitted nested pages, killed accepted reply, original-proof retry, durable ACK/prefix and holding-change stop passed"
    );
    Box::pin(pipeline::run(&root, 2, Duration::from_millis(10))).await?;
    Ok(())
}
