//! Keeps the room on Linux, as the machine that first ran short of it was
//! kept by hand. Every ten seconds, while less than the room and a little
//! more is available, part of what the node's own service holds in memory
//! is moved to swap through its control group's `memory.reclaim`, the kernel
//! taking the least used first, which is what idle sessions hold: at most
//! 384 MiB at a time, and none while memory already thrashes, when what is
//! moved out comes straight back; below the room itself, 768 MiB at a time
//! whether or not it thrashes. A node that may lower its own standing with
//! the kernel's out-of-memory killer also has it end the node's sessions
//! and jobs before the node itself, and before the machine's other services,
//! when memory and swap both run out. A node that is not its own service, or
//! whose kernel cannot reclaim a group's memory, keeps the room by waiting
//! turns alone.

use std::sync::Arc;

use crate::Node;

pub(crate) fn start(node: &Arc<Node>) {
    #[cfg(target_os = "linux")]
    {
        let node = node.clone();
        tokio::spawn(async move { linux::keep(node).await });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = node;
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::Node;

    const EVERY: Duration = Duration::from_secs(10);
    /// How much above the room the keeper aims for.
    const ABOVE: u64 = 300 << 20;
    const STEP: u64 = 384 << 20;
    const STEP_BELOW: u64 = 768 << 20;
    /// The share of the last ten seconds in which every task waited for
    /// memory, in percent, past which memory thrashes.
    const THRASHING: f32 = 10.0;
    /// How often what was moved is told in the log.
    const TOLD_EVERY: Duration = Duration::from_secs(60);
    /// The kernel's standing of the node, and of what it runs, with its
    /// out-of-memory killer: the node last, its sessions and jobs first.
    const NODE_STANDING: i32 = -500;
    const WORK_STANDING: i32 = 300;

    pub(super) async fn keep(node: Arc<Node>) {
        let Some(group) = own_group() else { return };
        if !group.join("memory.reclaim").exists() {
            node.log.line(
                "this kernel cannot move a control group's memory to swap: the room kept for agents is kept by waiting turns alone",
            );
            return;
        }
        let ranks = rank_self();
        let mut moved = 0u64;
        let mut told = Instant::now();
        let mut ranked: Option<Instant> = None;
        let mut interval = tokio::time::interval(EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let room = node.memory_room.bytes();
            if room == 0 {
                continue;
            }
            let rank = ranks && ranked.is_none_or(|at| at.elapsed() >= Duration::from_secs(60));
            if rank {
                ranked = Some(Instant::now());
            }
            let at = group.clone();
            let round = tokio::task::spawn_blocking(move || {
                if rank {
                    rank_work(&at);
                }
                reclaim(&at, room)
            })
            .await;
            match round {
                Ok(Ok(amount)) => moved += amount,
                Ok(Err(error)) => {
                    node.log.line(format_args!(
                        "moving what the node's service holds to swap: {error}; the room is kept by waiting turns alone"
                    ));
                    return;
                }
                Err(_) => {}
            }
            if moved > 0 && told.elapsed() >= TOLD_EVERY {
                node.log.line(format_args!(
                    "kept {} free for agents: moved {} the node's service held to swap",
                    super::super::size(room),
                    super::super::size(moved)
                ));
                moved = 0;
                told = Instant::now();
            }
        }
    }

    /// The node's own control group, when it is a service of its own.
    fn own_group() -> Option<PathBuf> {
        let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
        let name = Path::new(path).file_name()?.to_str()?;
        (name.starts_with("fastctx") && name.ends_with(".service"))
            .then(|| Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
    }

    /// Moves part of what the group holds to swap while less than the room
    /// and a little more is available; how much it asked to move.
    fn reclaim(group: &Path, room: u64) -> io::Result<u64> {
        let Some(available) = available()? else { return Ok(0) };
        let target = room + ABOVE;
        if available >= target {
            return Ok(0);
        }
        let below = available < room;
        if !below && thrashing() {
            return Ok(0);
        }
        let amount = (target - available).min(if below { STEP_BELOW } else { STEP });
        match std::fs::write(group.join("memory.reclaim"), amount.to_string()) {
            Ok(()) => Ok(amount),
            // Less could be moved this time; what was moved stays moved.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(error) => Err(error),
        }
    }

    fn available() -> io::Result<Option<u64>> {
        let text = std::fs::read_to_string("/proc/meminfo")?;
        Ok(text
            .lines()
            .find_map(|line| line.strip_prefix("MemAvailable:"))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kib| kib * 1024))
    }

    /// Whether every task waited for memory a tenth or more of the last ten
    /// seconds.
    fn thrashing() -> bool {
        std::fs::read_to_string("/proc/pressure/memory").ok().is_some_and(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("full "))
                .and_then(|rest| rest.split_whitespace().find_map(|field| field.strip_prefix("avg10=")))
                .and_then(|value| value.parse::<f32>().ok())
                .is_some_and(|share| share >= THRASHING)
        })
    }

    /// Lowers the node's own standing, where it may: only then is what it
    /// runs raised.
    fn rank_self() -> bool {
        std::fs::write("/proc/self/oom_score_adj", NODE_STANDING.to_string()).is_ok()
    }

    /// Raises what runs in the group, the node aside, that stands lower.
    fn rank_work(group: &Path) {
        let Ok(text) = std::fs::read_to_string(group.join("cgroup.procs")) else { return };
        let me = std::process::id();
        for pid in text.lines().filter_map(|line| line.trim().parse::<u32>().ok()).filter(|pid| *pid != me) {
            let path = PathBuf::from(format!("/proc/{pid}/oom_score_adj"));
            let standing = std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<i32>().ok());
            if standing.is_some_and(|standing| standing < WORK_STANDING) {
                // A process that ended meanwhile is no matter.
                let _ = std::fs::write(&path, WORK_STANDING.to_string());
            }
        }
    }
}
