//! Real TLS/WebSocket scale and reconnect drill. Run on Linux with --cgroup
//! to limit only the hub to two CPUs and 2 GiB; the generator stays outside.
//! No node, model host, login account or system installation is used.

// A drill drives the hub from outside it, and is not held to the roles of
// the hub's threads (`clippy.toml`).
#![allow(clippy::disallowed_methods)]

mod client;
mod fixture;
mod lifecycle;
mod validation;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

struct HubProcess {
    child: Child,
    unit: Option<String>,
}
impl Drop for HubProcess {
    fn drop(&mut self) {
        if let Some(unit) = &self.unit {
            let _ = Command::new("systemctl").args(["stop", unit]).status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn main() -> anyhow::Result<()> {
    fastctx_platform::limits::raise_open_files()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut threads = tokio::runtime::Builder::new_multi_thread();
    threads.worker_threads(if args.first().is_some_and(|arg| arg == "hub") { 2 } else { 4 }).max_blocking_threads(32);
    if args.first().is_some_and(|arg| arg == "hub") {
        let leave = fastctx_hub::Leave { remove_service: Box::new(|_| Ok(String::new())) };
        return fastctx_hub::run(Path::new(&args[1]), leave, threads);
    }
    let runtime = threads.enable_all().build()?;
    let value =
        |flag: &str| args.iter().position(|arg| arg == flag).and_then(|at| args.get(at + 1)).map(String::as_str);
    let root = PathBuf::from(
        value("--root").ok_or_else(|| anyhow::anyhow!("--root requires an isolated absolute directory"))?,
    );
    anyhow::ensure!(root.is_absolute(), "--root must be absolute");
    let count = value("--members").unwrap_or("10000").parse::<usize>()?;
    let seconds = value("--seconds").unwrap_or("30").parse::<u64>()?;
    anyhow::ensure!(count >= 2 && count % 2 == 0, "--members must be a positive even count");
    let cgroup = args.iter().any(|arg| arg == "--cgroup");
    anyhow::ensure!(!cgroup || cfg!(target_os = "linux"), "--cgroup needs Linux systemd");
    let fault = value("--fault").map(str::to_string);
    anyhow::ensure!(
        fault.as_deref().is_none_or(|f| matches!(f, "pong" | "letter" | "view")),
        "--fault must be pong, letter or view"
    );
    runtime.block_on(drill(root, count, seconds, cgroup, fault, value("--baseline").map(PathBuf::from)))
}

async fn drill(
    root: PathBuf,
    count: usize,
    seconds: u64,
    cgroup: bool,
    fault: Option<String>,
    baseline: Option<PathBuf>,
) -> anyhow::Result<()> {
    const STABLE: usize = 16;
    let run = root.join(format!("scale-{}", std::process::id()));
    std::fs::create_dir_all(&run)?;
    let (address, pin, identities) = fixture::prepare(&run, count, STABLE)?;
    let executable = std::env::current_exe()?;
    let unit = cgroup.then(|| format!("fastctx-hub-scale-{}", std::process::id()));
    let mut command = if let Some(unit) = &unit {
        let mut command = Command::new("systemd-run");
        command.args([
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "--unit",
            unit,
            "--property=CPUQuota=200%",
            "--property=MemoryMax=2G",
            "--property=MemorySwapMax=0",
            "--property=LimitNOFILE=131072",
        ]);
        command.arg(format!("--setenv=FASTCTX_HOME={}", run.join("private-home").display()));
        command.arg("--setenv=FASTCTX_HUB_HEALTH_SECONDS=1");
        command.arg(&executable);
        command
    } else {
        Command::new(&executable)
    };
    command
        .arg("hub")
        .arg(run.join("hub"))
        .env("FASTCTX_HOME", run.join("private-home"))
        .env("FASTCTX_HUB_HEALTH_SECONDS", "1")
        .stdout(Stdio::from(std::fs::File::create(run.join("process.log"))?))
        .stderr(Stdio::from(std::fs::File::create(run.join("process.err"))?));
    let mut process = HubProcess { child: command.spawn()?, unit };
    let ready = Instant::now();
    loop {
        anyhow::ensure!(process.child.try_wait()?.is_none(), "hub exited during startup; see {}", run.display());
        if tokio::net::TcpStream::connect(address).await.is_ok() {
            break;
        }
        anyhow::ensure!(ready.elapsed() < Duration::from_secs(60), "hub did not listen within 60 seconds");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let limits = process.unit.as_deref().map(|unit| cgroup_state(unit, &executable)).transpose()?;
    lifecycle::check(&run, identities.last().expect("controller").clone(), address, pin).await?;
    let (phase, receiver) = tokio::sync::watch::channel(0);
    let stats = Arc::new(client::Stats { fault, ..Default::default() });
    let slots = Arc::new(tokio::sync::Semaphore::new(128));
    let mut clients = tokio::task::JoinSet::new();
    let total = identities.len();
    let start = Instant::now();
    for (index, identity) in identities.into_iter().enumerate() {
        clients.spawn(client::run(
            index,
            count,
            identity,
            address,
            pin,
            receiver.clone(),
            slots.clone(),
            stats.clone(),
        ));
    }
    wait_for(&mut clients, Duration::from_secs(600), || stats.connected.load(Ordering::SeqCst) == total).await?;
    let initial_ms = start.elapsed().as_millis();
    wait_for(&mut clients, Duration::from_secs(120), || {
        stats.initially_viewed.load(Ordering::Relaxed) == total - 1
            && stats.initially_ponged.load(Ordering::Relaxed) == total
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let first_health = health(&run)?;
    phase.send_replace(1);
    wait_for(&mut clients, Duration::from_secs(30), || stats.connected.load(Ordering::SeqCst) == STABLE + 1).await?;
    let cut = stats.connected.load(Ordering::SeqCst);
    let cut_at = jiff::Timestamp::now();
    fresh_health(&run, &mut process, cut_at, STABLE + 1, false).await?;
    // Warm-up is paced, but the reconnect storm admits every member at once.
    slots.add_permits(count.saturating_sub(128));
    let reconnect = Instant::now();
    phase.send_replace(2);
    wait_for(&mut clients, Duration::from_secs(600), || stats.connected.load(Ordering::SeqCst) == total).await?;
    let reconnect_ms = reconnect.elapsed().as_millis();
    wait_for(&mut clients, Duration::from_secs(120), || {
        stats.reconnected_viewed.load(Ordering::Relaxed) == count
            && stats.reconnected_ponged.load(Ordering::Relaxed) == count
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    phase.send_replace(4);
    wait_for(&mut clients, Duration::from_secs(120), || stats.paused.load(Ordering::SeqCst) == total).await?;
    wait_for(&mut clients, Duration::from_secs(120), || {
        let sent = stats.letters_sent.load(Ordering::Relaxed);
        stats.letters_received.load(Ordering::Relaxed) == sent
            && stats.letters_held.load(Ordering::Relaxed) == sent
            && stats.pending_changes.load(Ordering::Relaxed) == 0
            && stats
                .observed_groups
                .iter()
                .all(|seen| seen.load(Ordering::Relaxed) >= stats.expected_group.load(Ordering::Relaxed))
    })
    .await?;
    let final_health = fresh_health(&run, &mut process, jiff::Timestamp::now(), total, true).await?;
    let resources = process.unit.as_deref().map(|unit| cgroup_state(unit, &executable)).transpose()?;
    phase.send_replace(3);
    while let Some(client) = clients.join_next().await {
        client??;
    }
    anyhow::ensure!(
        stats.letters_sent.load(Ordering::Relaxed) == stats.letters_received.load(Ordering::Relaxed)
            && stats.letters_sent.load(Ordering::Relaxed) == stats.letters_held.load(Ordering::Relaxed),
        "letter totals changed after the sender pause barrier: sent {}, received {}, held {}",
        stats.letters_sent.load(Ordering::Relaxed),
        stats.letters_received.load(Ordering::Relaxed),
        stats.letters_held.load(Ordering::Relaxed)
    );
    let result = serde_json::json!({
        "members_reconnected": count, "stable_members": STABLE, "connected_while_cut": cut,
        "reconnect_parallelism": count.max(128), "peak_simultaneous_dials": stats.peak_dials.load(Ordering::SeqCst), "handshake_retries": stats.handshake_retries.load(Ordering::Relaxed),
        "cgroup": cgroup, "enforced_limits": limits, "resources": resources, "initial_ms": initial_ms, "reconnect_ms": reconnect_ms,
        "unexpected_disconnects": stats.unexpected.load(Ordering::Relaxed), "stable_disconnected": stats.stable_lost.load(Ordering::Relaxed),
        "views": stats.views.load(Ordering::Relaxed), "initially_viewed": stats.initially_viewed.load(Ordering::Relaxed), "reconnected_viewed": stats.reconnected_viewed.load(Ordering::Relaxed),
        "expected_group": stats.expected_group.load(Ordering::Relaxed), "observed_groups": stats.observed_groups.iter().map(|group| group.load(Ordering::Relaxed)).collect::<Vec<_>>(), "wire_replays": stats.wire_replays.load(Ordering::Relaxed), "directory_changes": stats.changes.load(Ordering::Relaxed), "view_deltas": stats.deltas.load(Ordering::Relaxed),
        "pongs": stats.pongs.load(Ordering::Relaxed), "longest_pong_gap_ms": stats.longest_pong_gap_ms.load(Ordering::Relaxed),
        "longest_view_ms": stats.longest_view_ms.load(Ordering::Relaxed),
        "letters_sent": stats.letters_sent.load(Ordering::Relaxed), "letters_held": stats.letters_held.load(Ordering::Relaxed), "letters_received": stats.letters_received.load(Ordering::Relaxed),
        "before_storm": first_health, "after_storm": final_health,
    });
    if let Some(path) = baseline {
        let baseline = serde_json::from_slice(&std::fs::read(path)?)?;
        validation::compare(&baseline, &result)?;
    }
    std::fs::write(root.join(format!("result-{count}.json")), serde_json::to_vec_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    anyhow::ensure!(
        stats.stable_lost.load(Ordering::Relaxed) == 0 && stats.unexpected.load(Ordering::Relaxed) == 0,
        "healthy members disconnected"
    );
    anyhow::ensure!(reconnect_ms <= 600_000, "reconnection took more than ten minutes");
    anyhow::ensure!(
        stats.changes.load(Ordering::Relaxed) > 0
            && stats.deltas.load(Ordering::Relaxed) > 0
            && stats.pongs.load(Ordering::Relaxed) >= total - 1
            && stats.letters_received.load(Ordering::Relaxed) > 0,
        "the intended load did not run"
    );
    anyhow::ensure!(
        stats.longest_pong_gap_ms.load(Ordering::Relaxed) <= 12_000,
        "heartbeat gap exceeded twelve seconds"
    );
    let locks = final_health["locks"].as_array().ok_or_else(|| anyhow::anyhow!("hub omitted lock measurements"))?;
    for name in [
        "hub sessions",
        "hub given",
        "hub calls",
        "hub data",
        "hub ledgers",
        "hub afoots",
        "hub updates",
        "hub presence",
    ] {
        anyhow::ensure!(
            locks.iter().any(|lock| lock["name"].as_str() == Some(name)),
            "hub omitted {name} measurements"
        );
    }
    for name in ["hub sessions", "hub given", "hub updates", "hub presence"] {
        anyhow::ensure!(
            locks
                .iter()
                .find(|lock| lock["name"].as_str() == Some(name))
                .is_some_and(|lock| lock["holding"]["count"].as_u64().unwrap_or(0) > 0),
            "{name} had no measurement samples"
        );
    }
    for lock in final_health["locks"].as_array().ok_or_else(|| anyhow::anyhow!("hub omitted lock measurements"))? {
        anyhow::ensure!(
            lock["holding"]["p99_us"].as_u64().is_some_and(|n| n <= 1000),
            "shared lock p99 exceeded one millisecond: {lock}"
        );
    }
    anyhow::ensure!(final_health["lock_order_violations"].as_u64() == Some(0), "lock order violations occurred");
    anyhow::ensure!(
        final_health["violations"].as_u64() == Some(0),
        "the hub broke the threads' rules: {}",
        final_health["violated"]
    );
    println!(
        "PASS hub scale: {count} reconnected in {reconnect_ms} ms, {STABLE} continuously connected members stayed online"
    );
    Ok(())
}

async fn wait_for(
    clients: &mut tokio::task::JoinSet<anyhow::Result<()>>,
    limit: Duration,
    ready: impl Fn() -> bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        if ready() {
            return Ok(());
        }
        if let Some(client) = clients.try_join_next() {
            client??;
            anyhow::bail!("a client ended before the drill stopped");
        }
        anyhow::ensure!(start.elapsed() < limit, "scale phase did not complete in {} seconds", limit.as_secs());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn health(run: &Path) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_slice(&std::fs::read(run.join("hub/health.json"))?)?)
}

async fn fresh_health(
    run: &Path,
    process: &mut HubProcess,
    after: jiff::Timestamp,
    sessions: usize,
    drained: bool,
) -> anyhow::Result<serde_json::Value> {
    let began = Instant::now();
    loop {
        anyhow::ensure!(process.child.try_wait()?.is_none(), "hub exited before final health");
        let health = health(run)?;
        let at: jiff::Timestamp =
            health["at"].as_str().ok_or_else(|| anyhow::anyhow!("health lacks timestamp"))?.parse()?;
        let empty = [
            (&health["queues"], "urgent_frames"),
            (&health["queues"], "letters"),
            (&health["queues"], "notices"),
            (&health["letters"], "held"),
            (&health["letters"], "on_their_way"),
        ]
        .into_iter()
        .all(|(object, field)| object[field].as_u64() == Some(0));
        if at > after && health["sessions"].as_u64() == Some(sessions as u64) && (!drained || empty) {
            return Ok(health);
        }
        anyhow::ensure!(
            began.elapsed() < Duration::from_secs(45),
            "hub did not report fresh {} sessions{}",
            sessions,
            if drained { " with drained queues" } else { "" }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn cgroup_state(unit: &str, executable: &Path) -> anyhow::Result<serde_json::Value> {
    let output =
        Command::new("systemctl").args(["show", unit, "--property=MainPID", "--property=ControlGroup"]).output()?;
    anyhow::ensure!(output.status.success(), "could not inspect the hub cgroup");
    let text = String::from_utf8(output.stdout)?;
    let property = |name: &str| text.lines().find_map(|line| line.strip_prefix(name));
    let pid: u32 = property("MainPID=").ok_or_else(|| anyhow::anyhow!("unit has no PID"))?.parse()?;
    let group = property("ControlGroup=").ok_or_else(|| anyhow::anyhow!("unit has no cgroup"))?;
    anyhow::ensure!(pid != 0 && group.starts_with('/'), "hub unit is not running");
    anyhow::ensure!(
        std::fs::read_link(format!("/proc/{pid}/exe"))?.canonicalize()? == executable.canonicalize()?,
        "cgroup contains a different executable"
    );
    anyhow::ensure!(
        std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?.lines().any(|line| line == format!("0::{group}")),
        "hub PID is not in the constrained cgroup"
    );
    let root = PathBuf::from("/sys/fs/cgroup").join(group.trim_start_matches('/'));
    let cpu = std::fs::read_to_string(root.join("cpu.max"))?;
    let quota: Vec<_> = cpu.split_whitespace().map(str::parse::<u64>).collect::<Result<_, _>>()?;
    anyhow::ensure!(quota.len() == 2 && quota[0] == 2 * quota[1], "hub CPU quota is not two CPUs: {cpu}");
    let memory = std::fs::read_to_string(root.join("memory.max"))?;
    let swap = std::fs::read_to_string(root.join("memory.swap.max"))?;
    anyhow::ensure!(memory.trim() == "2147483648" && swap.trim() == "0", "hub memory limit is not 2 GiB without swap");
    Ok(
        serde_json::json!({ "pid": pid, "group": group, "cpu_max": cpu.trim(), "memory_max": memory.trim(), "swap_max": swap.trim(),
        "memory_current": std::fs::read_to_string(root.join("memory.current"))?.trim(),
        "memory_peak": std::fs::read_to_string(root.join("memory.peak")).ok().map(|text| text.trim().to_owned()),
        "memory_events": std::fs::read_to_string(root.join("memory.events"))?, "cpu_stat": std::fs::read_to_string(root.join("cpu.stat"))? }),
    )
}
