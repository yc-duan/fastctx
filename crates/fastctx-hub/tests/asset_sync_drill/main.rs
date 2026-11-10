//! Two real processes discover and install signed scopes over the gated asset
//! connection. All keys, memberships and databases belong to this isolated run.
#![allow(clippy::disallowed_methods)]
mod chain;
mod fixture;
mod node;
use fastctx_sync::{
    metrics::PlatformMetrics,
    replicate::DirectoryFollower,
    store::{control, scopes::SqliteScopes},
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.worker_threads(2).max_blocking_threads(16).thread_stack_size(8 * 1024 * 1024);
    if args.first().is_some_and(|a| a == "chain-hub") {
        return chain::hub(Path::new(&args[1]), runtime);
    }
    if args.first().is_some_and(|a| a == "chain-a" || a == "chain-b" || a == "chain-perf") {
        return runtime.enable_all().build()?.block_on(chain::child(&args));
    }
    if args.first().is_some_and(|a| a == "hub") {
        return fastctx_hub::run(
            Path::new(&args[1]),
            fastctx_hub::Leave { remove_service: Arc::new(|_| Ok(String::new())) },
            runtime,
        );
    }
    let root = PathBuf::from(
        args.windows(2)
            .find(|a| a[0] == "--root")
            .map(|a| a[1].clone())
            .ok_or_else(|| anyhow::anyhow!("--root requires an isolated absolute directory"))?,
    );
    anyhow::ensure!(root.is_absolute() && !root.exists(), "drill root must be a new absolute directory");
    if args.iter().any(|a| a == "--chain" || a == "--benchmark") {
        return runtime.enable_all().build()?.block_on(chain::run(&root, args.iter().any(|a| a == "--benchmark")));
    }
    runtime.enable_all().build()?.block_on(drill(root))
}
async fn drill(root: PathBuf) -> anyhow::Result<()> {
    let fixture = fixture::prepare(&root).await?;
    let mut hub = Process(
        Command::new(std::env::current_exe()?)
            .arg("hub")
            .arg(root.join("hub"))
            .env("FASTCTX_HOME", root.join("private-home"))
            .stdout(Stdio::from(std::fs::File::create(root.join("hub.out"))?))
            .stderr(Stdio::from(std::fs::File::create(root.join("hub.err"))?))
            .spawn()?,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        anyhow::ensure!(hub.0.try_wait()?.is_none(), "hub stopped; see hub.out/hub.err");
        if tokio::net::TcpStream::connect(fixture.address).await.is_ok() {
            break;
        }
        anyhow::ensure!(tokio::time::Instant::now() < deadline, "hub startup deadline");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let device = fastctx_sync::identity::DeviceId::from_bytes(*fixture.key.public().as_bytes());
    let blocks = fixture::blocks(&root.join("node"), fastctx_sync::cursor::Holder::Device { device: device.clone() })?;
    let membership = fastctx_net::link::Membership {
        id: fixture.hub.clone(),
        hub_name: "private-test".parse().unwrap(),
        name: "node".parse().unwrap(),
        addresses: vec![fixture.address],
        cert: fixture.pin,
        kind: fastctx_net::directory::Kind::Phone,
        relays: Vec::new(),
    };
    let link = fastctx_net::link::Link::start_managed(
        fastctx_net::link::NetDir::new(root.join("node-link")),
        membership,
        fixture.key,
        Arc::new(node::Node),
    )?;
    let upstream = tokio::time::timeout(Duration::from_secs(60), link.asset_upstream(blocks.buffer_budget())).await??;
    let identity = upstream.identity().clone();
    anyhow::ensure!(identity == fixture.identity, "authenticated control advertised another signing identity");
    control::pin_key(&blocks, &identity).await?;
    let verifier = Arc::new(fastctx_net::sync_proof::IdentityVerifier::new(
        identity.hub.clone(),
        [identity.key.clone()],
        Arc::new(PlatformMetrics),
    ));
    let scopes = SqliteScopes::new(blocks.clone(), fastctx_sync::identity::HubId::from_bytes([77; 8]), device);
    let follower = DirectoryFollower::new(scopes, identity, verifier);
    let discovered = tokio::time::timeout(Duration::from_secs(60), follower.discover(&upstream)).await??;
    anyhow::ensure!(discovered.installed.len() == fixture::CONVERSATIONS + 1, "not all signed scopes were installed");
    anyhow::ensure!(discovered.directories.len() == 1, "discovery lost the account directory cursor");
    let repeated = follower.discover(&upstream).await?;
    anyhow::ensure!(repeated.installed.len() == discovered.installed.len(), "second discovery changed the scope set");
    drop(upstream);
    anyhow::ensure!(blocks.buffer_budget().reserved() == 0, "asset channel or metadata leaked its runtime reservation");
    let installed = fastctx_storage::Database::open(&root.join("node/node.sqlite3"), "")?.read_now(|db| {
        Ok(db.query_row("SELECT count(*) FROM sync_scopes WHERE enabled=1", [], |r| r.get::<_, i64>(0))?)
    })?;
    anyhow::ensure!(
        installed == (fixture::CONVERSATIONS + 1) as i64,
        "installed scope rows differ from discovery receipts"
    );
    link.stop();
    println!(
        "asset sync lifecycle: two real processes, pinned TLS, device proof, mutual capability gate, control-bound data socket, multi-frame paged signed discovery, idempotent installation and zero retained runtime bytes passed"
    );
    Ok(())
}
