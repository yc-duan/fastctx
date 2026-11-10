//! Isolated real processes exercise durable source upload and replica continuation.
mod config;
mod data;
mod node;
mod policy;
mod producer;
mod proxy;
mod receiver;
mod retry;
use config::Config;
use std::{path::Path, process::Command, sync::Arc, time::Duration};
const KIND: &str = "conversation.timeline";

pub(crate) fn hub(root: &Path, runtime: tokio::runtime::Builder) -> anyhow::Result<()> {
    let config = Config::load(root)?;
    fastctx_hub::run_with_foundation(
        &root.join("hub"),
        fastctx_hub::Leave { remove_service: Arc::new(|_| Ok(String::new())) },
        runtime,
        config.installation(),
    )
}
pub(crate) async fn child(args: &[String]) -> anyhow::Result<()> {
    let root = Path::new(&args[1]);
    match args[0].as_str() {
        "chain-a" => producer::run(root, args[2] == "pause").await,
        "chain-perf" => benchmark_existing(root, &args[2]).await,
        "chain-b" => receiver::run(root, args[2].parse()?, args[3] == "pause", &args[4], args[5] == "revoke").await,
        _ => anyhow::bail!("unknown isolated child mode"),
    }
}
fn spawn(root: &Path, name: &str, mode: &str, args: &[&str]) -> anyhow::Result<crate::Process> {
    Ok(crate::Process(
        Command::new(std::env::current_exe()?)
            .arg(mode)
            .arg(root)
            .args(args)
            .env("FASTCTX_HOME", root.join("private-home"))
            .stdout(std::fs::File::create(root.join(format!("{name}.out")))?)
            .stderr(std::fs::File::create(root.join(format!("{name}.err")))?)
            .spawn()?,
    ))
}
async fn wait(child: &mut crate::Process, root: &Path, name: &str) -> anyhow::Result<()> {
    let limit = tokio::time::Instant::now() + Duration::from_secs(1200);
    loop {
        if let Some(exit) = child.0.try_wait()? {
            anyhow::ensure!(
                exit.success(),
                "{name} exited {exit}: {}",
                std::fs::read_to_string(root.join(format!("{name}.err")))?
            );
            return Ok(());
        }
        anyhow::ensure!(tokio::time::Instant::now() < limit, "{name} deadline");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn kill_at(child: &mut crate::Process, root: &Path, marker: &str) -> anyhow::Result<()> {
    let limit = tokio::time::Instant::now() + Duration::from_secs(600);
    while !root.join(marker).exists() {
        anyhow::ensure!(child.0.try_wait()?.is_none(), "child stopped before {marker}");
        anyhow::ensure!(tokio::time::Instant::now() < limit, "child did not reach {marker}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child.0.kill()?;
    anyhow::ensure!(!child.0.wait()?.success(), "child was not forcibly stopped");
    Ok(())
}
type StoredRecord = (Vec<u8>, String, Vec<u8>, String, String);
fn rows(path: &Path) -> anyhow::Result<Vec<StoredRecord>> {
    let db = fastctx_storage::rusqlite::Connection::open(path)?;
    Ok(db
        .prepare(
            "SELECT sequence,operation,payload,batch_proof,prefix_digest
        FROM sync_stream_records WHERE kind='conversation.timeline' ORDER BY stream,sequence",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}
pub(crate) async fn run(root: &Path, benchmark: bool) -> anyhow::Result<()> {
    let config = Config::prepare(root).await?;
    data::prepare(root, &config, if benchmark { 256 } else { 4 }).await?;
    let mut hub = spawn(root, "chain-hub", "chain-hub", &[])?;
    let limit = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::net::TcpStream::connect(config.address).await.is_err() {
        anyhow::ensure!(hub.0.try_wait()?.is_none(), "hub stopped");
        anyhow::ensure!(tokio::time::Instant::now() < limit, "hub startup deadline");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut producer = spawn(root, "a-interrupted", "chain-a", &["pause"])?;
    kill_at(&mut producer, root, "pause-a").await?;
    let mut resumed = spawn(root, "a-resumed", "chain-a", &["resume"])?;
    wait(&mut resumed, root, "a-resumed").await?;
    let mut receiver = spawn(root, "b-interrupted", "chain-b", &["1", "pause", "lifecycle", "keep"])?;
    kill_at(&mut receiver, root, "pause-b").await?;
    let mut resumed = spawn(root, "b-resumed", "chain-b", &["1", "resume", "lifecycle", "keep"])?;
    wait(&mut resumed, root, "b-resumed").await?;
    let expected = rows(&root.join("hub/hub.sqlite3"))?;
    anyhow::ensure!(expected.len() == 4, "hub source duplicate or gap");
    anyhow::ensure!(
        rows(&root.join("node-0/node.sqlite3"))? == expected,
        "producer prefix differs from accepted prefix"
    );
    anyhow::ensure!(
        rows(&root.join("node-1-lifecycle/node.sqlite3"))? == expected,
        "replica records differ after restart"
    );
    if benchmark {
        performance(root, &config, Duration::ZERO, "loopback", 1).await?;
        performance(root, &config, Duration::ZERO, "loopback-four", 4).await?;
        performance(root, &config, Duration::from_millis(25), "rtt50-four", 4).await?;
    }
    let mut revoke = spawn(root, "b-revoke", "chain-b", &["1", "resume", "lifecycle", "revoke"])?;
    wait(&mut revoke, root, "b-revoke").await?;
    println!(
        "real source chain: A SourceUploader -> authenticated TLS -> FULL hub acceptance -> B ReplicaFollower/content; killed A before receipt, killed B after one durable record, exact prefixes, complete closure, logout revocation passed"
    );
    Ok(())
}
async fn performance(root: &Path, c: &Config, delay: Duration, phase: &str, devices: usize) -> anyhow::Result<()> {
    let proxy = proxy::Proxy::start(c.address, delay).await?;
    let mut wire = Config::load(root)?;
    wire.address = proxy.address;
    std::fs::write(root.join("chain.json"), serde_json::to_vec(&wire)?)?;
    let began = std::time::Instant::now();
    let mut children = Vec::new();
    for n in 1..=devices {
        children.push(spawn(root, &format!("{phase}-{n}"), "chain-b", &[&n.to_string(), "resume", phase, "keep"])?);
    }
    for (i, child) in children.iter_mut().enumerate() {
        wait(child, root, &format!("{phase}-{}", i + 1)).await?;
    }
    let mut bytes = 0_u64;
    let mut busy = 0_u64;
    let mut unavailable = 0_u64;
    for n in 1..=devices {
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(format!("result-{n}-{phase}.json")))?)?;
        bytes += result["bytes"].as_u64().unwrap();
        busy += result["busy"].as_u64().unwrap();
        unavailable += result["unavailable"].as_u64().unwrap_or(0);
    }
    let seconds = began.elapsed().as_secs_f64();
    println!(
        "{phase}: devices={devices}, slots=3/device, bytes={bytes}, seconds={seconds:.3}, aggregate={:.2}MiB/s, Busy retries={busy}, unavailable retries={unavailable}, TLS sockets={}",
        bytes as f64 / 1048576.0 / seconds,
        proxy.connections()
    );
    std::fs::write(root.join("chain.json"), serde_json::to_vec(c)?)?;
    Ok(())
}

async fn benchmark_existing(root: &Path, generation: &str) -> anyhow::Result<()> {
    let mut c = Config::load(root)?;
    anyhow::ensure!(
        data::load(root)?.bytes == 1024 * 1024 * 1024,
        "requires the completed private 1GiB source fixture"
    );
    c.address = fastctx_hub::describe(&root.join("hub"))?.0.listen;
    std::fs::write(root.join("chain.json"), serde_json::to_vec(&c)?)?;
    let mut hub = spawn(root, "benchmark-hub", "chain-hub", &[])?;
    let limit = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::net::TcpStream::connect(c.address).await.is_err() {
        anyhow::ensure!(hub.0.try_wait()?.is_none(), "benchmark hub stopped");
        anyhow::ensure!(tokio::time::Instant::now() < limit, "benchmark hub startup deadline");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    performance(root, &c, Duration::ZERO, &format!("loopback-single-{generation}"), 1).await?;
    performance(root, &c, Duration::ZERO, &format!("loopback-four-{generation}"), 4).await?;
    performance(root, &c, Duration::from_millis(25), &format!("rtt50-four-{generation}"), 4).await?;
    Ok(())
}
