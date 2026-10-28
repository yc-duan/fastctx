//! How this machine is doing, for its details page (`world-gui.md`, 机器的
//! 详情), and asking another machine the same. Only an account that may
//! work on a machine is told (`RemoteCall::Stats` asks for its hands).
//!
//! Each answer is read when it is asked for; nothing is sampled while no
//! one looks. Rates are measured against the previous answer when it is
//! recent, which a page that asks every few seconds always finds; else two
//! readings half a second apart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_net::link::Status;
use fastctx_platform::system::{CpuTimes, Traffic};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::stats::{Card, Cpu, Disk, Gpu, Line, MachineStats, Memory, NodeSeen, StatsSeen};
use jiff::Timestamp;

use crate::Node;

/// How `machine`, or this computer, is doing; its lines' history only
/// after `after`, when given.
pub(crate) async fn query(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<Written>,
    after: Option<Timestamp>,
    seen: Option<StatsSeen>,
) -> Result<MachineStats, Failure> {
    let Some(target) = machine.filter(|machine| !node.is_here(machine)) else {
        return here(node, after, seen).await;
    };
    match node
        .call(acting, &target, Purpose::Hands, RemoteCall::Stats { after, seen })
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Stats(stats) => Ok(*stats),
        _ => Err(Failure::new(format!("{target} answered something other than how it is doing"))),
    }
}

/// The counters of one reading, which the next measures rates against.
#[derive(Clone)]
struct Counters {
    at: Instant,
    cpu: Option<CpuTimes>,
    cards: HashMap<u32, (u64, u64)>,
}

static LAST: Mutex<Option<Counters>> = Mutex::new(None);

/// This run of the node, which the number of a line's round trips and
/// changes belongs to.
fn run() -> String {
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUN.get_or_init(|| format!("{:x}", Timestamp::now().as_nanosecond())).clone()
}

/// How long apart two readings are taken when there was none lately: long
/// enough for the processors' counters to have moved.
const GLANCE: Duration = Duration::from_millis(500);

/// A previous reading older than this measures an average over too long to
/// say how the machine is doing now; closer than `SHORTEST`, too little.
const STALE: Duration = Duration::from_secs(60);
const SHORTEST: Duration = Duration::from_millis(200);

/// How long the graphics cards' reading stands: their tool takes a moment
/// to start, and a page asking every two seconds would keep it busy.
const GPU_FOR: Duration = Duration::from_secs(5);

static GPUS: Mutex<Option<(Instant, Vec<Gpu>)>> = Mutex::new(None);

/// Whether the machine has NVIDIA's tool at all; asked once.
static GPU_TOOL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn counters(traffic: &[Traffic]) -> Counters {
    Counters {
        at: Instant::now(),
        cpu: fastctx_platform::system::cpu_times().ok(),
        cards: traffic.iter().map(|card| (card.index, (card.received, card.sent))).collect(),
    }
}

/// How this computer is doing now; its lines' history only after `after`,
/// when given.
pub(crate) async fn here(
    node: &Arc<Node>,
    after: Option<Timestamp>,
    seen: Option<StatsSeen>,
) -> Result<MachineStats, Failure> {
    let read = || async {
        tokio::task::spawn_blocking(|| {
            let traffic = fastctx_platform::system::traffic().unwrap_or_default();
            (counters(&traffic), traffic)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))
    };
    let (mut now, mut traffic) = read().await?;
    let previous = LAST.lock().expect("never poisoned").clone().filter(|before| {
        now.at.checked_duration_since(before.at).is_some_and(|gone| gone >= SHORTEST && gone <= STALE)
    });
    let before = match previous {
        Some(before) => before,
        None => {
            tokio::time::sleep(GLANCE).await;
            let first = now;
            (now, traffic) = read().await?;
            first
        }
    };
    *LAST.lock().expect("never poisoned") = Some(now.clone());
    let seconds = now.at.duration_since(before.at).as_secs_f64();
    let busy = match (now.cpu, before.cpu) {
        (Some(now), Some(before)) => now.busy_since(&before),
        _ => None,
    };
    let rate = |index: u32, bytes: u64, of: fn(&(u64, u64)) -> u64| -> Option<f64> {
        let earlier = of(before.cards.get(&index)?);
        Some(bytes.checked_sub(earlier)? as f64 / seconds)
    };
    let gpus = gpus().await;
    let (system, kernel, memory, disks, uptime, model, interfaces) = tokio::task::spawn_blocking(|| {
        (
            fastctx_platform::system::system_name().unwrap_or_default(),
            fastctx_platform::system::kernel(),
            fastctx_platform::system::memory().ok(),
            fastctx_platform::system::disks().unwrap_or_default(),
            fastctx_platform::system::uptime().ok(),
            fastctx_platform::system::cpu_model().unwrap_or_default(),
            fastctx_net::route::interfaces().unwrap_or_default(),
        )
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    let at = Timestamp::now();
    let addresses: HashMap<u32, Vec<std::net::IpAddr>> = interfaces
        .iter()
        .map(|interface| {
            let addresses = interface
                .ipv4
                .iter()
                .filter(|address| !address.is_link_local())
                .map(|address| std::net::IpAddr::V4(*address))
                .chain(
                    interface
                        .ipv6
                        .iter()
                        .filter(|address| (address.segments()[0] & 0xffc0) != 0xfe80)
                        .map(|address| std::net::IpAddr::V6(*address)),
                )
                .collect();
            (interface.index, addresses)
        })
        .collect();
    // A card of the machine's own, up or not, and any other that is up with
    // an address: a virtual switch or a tunnel carries what the machine
    // sends too. The busiest first.
    let mut cards: Vec<Card> = traffic
        .into_iter()
        .filter_map(|card| {
            let addresses = addresses.get(&card.index).cloned().unwrap_or_default();
            if !card.hardware && (!card.up || addresses.is_empty()) {
                return None;
            }
            Some(Card {
                receiving: rate(card.index, card.received, |pair| pair.0),
                sending: rate(card.index, card.sent, |pair| pair.1),
                name: card.name,
                up: card.up,
                hardware: card.hardware,
                speed: card.speed,
                received: card.received,
                sent: card.sent,
                addresses,
            })
        })
        .collect();
    cards.sort_by(|a, b| {
        b.up.cmp(&a.up)
            .then(
                (b.receiving.unwrap_or(0.0) + b.sending.unwrap_or(0.0))
                    .total_cmp(&(a.receiving.unwrap_or(0.0) + a.sending.unwrap_or(0.0))),
            )
            .then((b.received + b.sent).cmp(&(a.received + a.sent)))
    });
    // What came after the reading before: by how many of each it was told,
    // in this run of the node; asked by an older interface, by when it was,
    // all of it again when that is later than now, this machine's clock
    // having gone back since.
    let fresh = |when: Timestamp| after.is_none_or(|after| when > after || after > at);
    let run = run();
    let seen = seen.filter(|seen| seen.run == run);
    let lines = node
        .links()
        .iter()
        .map(|link| {
            let (up, since, through) = match link.status() {
                Status::Online { since, through, .. } => (true, since, through),
                Status::Connecting { since, .. } => (false, since, None),
                Status::Stopped { .. } => (false, at, None),
            };
            let hub = link.hub_name();
            let (trips, changes, kept) = match &seen {
                // A line the last reading did not tell is told whole.
                Some(seen) => link.line_after(seen.lines.iter().find(|(line, _)| *line == hub).map(|(_, kept)| *kept)),
                None => {
                    let (trips, changes, kept) = link.line_after(None);
                    (
                        trips.into_iter().filter(|trip| fresh(trip.at)).collect(),
                        changes.into_iter().filter(|change| fresh(change.at())).collect(),
                        kept,
                    )
                }
            };
            Line { hub, kept: Some(kept), since, up, through, trips, changes }
        })
        .collect();
    let jobs = {
        let node = node.clone();
        tokio::task::spawn_blocking(move || crate::job_accounts::counts(&node)).await.unwrap_or_default()
    };
    Ok(MachineStats {
        run: Some(run),
        machine: node.machine(),
        at,
        boot: uptime.and_then(|uptime| at.checked_sub(jiff::SignedDuration::try_from(uptime).ok()?).ok()),
        system,
        kernel,
        host: fastctx_platform::hostname().unwrap_or_default(),
        arch: std::env::consts::ARCH.to_string(),
        cpu: Cpu { model, threads: fastctx_platform::system::threads(), busy, load: fastctx_platform::system::load() },
        memory: memory
            .map(|memory| Memory {
                total: memory.total,
                used: memory.used,
                swap_total: memory.swap_total,
                swap_used: memory.swap_used,
            })
            .unwrap_or(Memory { total: 0, used: 0, swap_total: 0, swap_used: 0 }),
        disks: disks
            .into_iter()
            .map(|disk| Disk {
                at: crate::world::native(&disk.at),
                label: disk.label,
                fs: disk.fs,
                total: disk.total,
                free: disk.free,
            })
            .collect(),
        cards,
        gpus,
        lines,
        node: NodeSeen {
            version: env!("CARGO_PKG_VERSION").to_string(),
            started: node.started,
            service: node.lifetime.supervised,
            health: crate::usage::health(node),
            stuck: node
                .stuck
                .now()
                .into_iter()
                .map(|stuck| fastctx_types::stats::StuckWork {
                    doing: stuck.doing,
                    what: stuck.what,
                    seconds: stuck.since.as_secs(),
                })
                .collect(),
            refused: fastctx_platform::privacy::last().and_then(|(path, at)| {
                Some(fastctx_types::stats::Refused { path, at: jiff::Timestamp::try_from(at).ok()? })
            }),
        },
        jobs,
    })
}

/// The machine's NVIDIA graphics cards, as `nvidia-smi` tells; none where
/// it is not installed.
async fn gpus() -> Vec<Gpu> {
    if let Some((at, gpus)) = &*GPUS.lock().expect("never poisoned")
        && at.elapsed() < GPU_FOR
    {
        return gpus.clone();
    }
    if GPU_TOOL.get() == Some(&false) {
        return Vec::new();
    }
    let read = tokio::task::spawn_blocking(|| {
        let mut command = std::process::Command::new("nvidia-smi");
        command
            .args(["--query-gpu=name,memory.total,memory.used,utilization.gpu", "--format=csv,noheader,nounits"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            /// `CREATE_NO_WINDOW`: no console flashes up for it.
            const NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(NO_WINDOW);
        }
        fastctx_platform::bounded::output(command, "nvidia-smi", std::time::Duration::from_secs(10))
    })
    .await;
    let gpus = match read {
        Ok(Ok(output)) if output.status.success() => {
            let _ = GPU_TOOL.set(true);
            String::from_utf8_lossy(&output.stdout).lines().filter_map(gpu_line).collect()
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = GPU_TOOL.set(false);
            Vec::new()
        }
        _ => Vec::new(),
    };
    *GPUS.lock().expect("never poisoned") = Some((Instant::now(), gpus.clone()));
    gpus
}

/// One card's line: its name, total and used memory in mebibytes, and how
/// busy it is in percent; a value the card does not report is `[N/A]`.
fn gpu_line(line: &str) -> Option<Gpu> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [name, total, used, busy] = fields.as_slice() else { return None };
    let mebibytes = |field: &str| field.parse::<u64>().ok().map(|mib| mib * 1024 * 1024);
    Some(Gpu {
        name: (*name).to_string(),
        memory_total: mebibytes(total).unwrap_or(0),
        memory_used: mebibytes(used).unwrap_or(0),
        busy: busy.parse::<f32>().ok().map(|percent| (percent / 100.0).clamp(0.0, 1.0)),
    })
}
