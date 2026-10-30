//! A service's aggregate cgroup owns both deployment layouts. The kernel's
//! anonymous-only capability is probed without reclaiming any pages. Numeric
//! swappiness, even 200, can reclaim file cache on older classic and MGLRU LRUs.
//! A kernel without it is asked plainly: each step is small, and one that
//! moves too little into swap, file cache taken instead, rests the keeper.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LEAST: u64 = 16 << 20;
const STEP: u64 = 128 << 20;
const ABOVE: u64 = 300 << 20;
const FIRST_REST: Duration = Duration::from_secs(60);
const MAX_REST: Duration = Duration::from_secs(600);

pub(super) struct Observation {
    pub state: &'static str,
    pub requested: u64,
    pub swapped: u64,
    pub available: u64,
}

pub(super) struct Keeper {
    group: PathBuf,
    unavailable: Option<String>,
    /// The kernel takes no `swappiness=max`: reclaim is asked plainly.
    plain: bool,
    ranked: Option<Instant>,
    standing: bool,
    page: u64,
    last: Option<(u64, u64)>,
    rest_until: Option<Instant>,
    rest: Duration,
}

impl Keeper {
    pub(super) fn new() -> io::Result<Self> {
        unsafe extern "C" {
            fn getpagesize() -> std::os::raw::c_int;
        }
        // SAFETY: getpagesize takes no pointers and has no preconditions.
        let page = unsafe { getpagesize() };
        if page <= 0 {
            return Err(io::Error::other("the system page size is unavailable"));
        }
        let page = page as u64;
        let group = service_group()?;
        let (unavailable, plain) = match fs::write(group.join("memory.reclaim"), "0 swappiness=max") {
            Ok(()) => (None, false),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => (None, true),
            Err(error) => (
                Some(match error.kind() {
                    io::ErrorKind::NotFound => "the service has no cgroup v2 memory.reclaim interface".into(),
                    io::ErrorKind::PermissionDenied => "the service account cannot write memory.reclaim".into(),
                    _ => format!("probing memory.reclaim: {error}"),
                }),
                false,
            ),
        };
        let standing = fs::write("/proc/self/oom_score_adj", "-500").is_ok();
        Ok(Self {
            group,
            unavailable,
            plain,
            ranked: None,
            standing,
            page,
            last: None,
            rest_until: None,
            rest: FIRST_REST,
        })
    }

    pub(super) fn description(&self) -> String {
        format!(
            "{}; {}; {}",
            self.group.display(),
            self.unavailable.as_deref().unwrap_or(if self.plain {
                "plain reclaim, resting when it swaps too little: the kernel cannot reclaim anonymous memory exclusively"
            } else {
                "anonymous-only reclaim available"
            }),
            if self.standing {
                "node OOM priority -500; service descendants 300"
            } else {
                "OOM priority unchanged: the account cannot lower its own score"
            }
        )
    }

    pub(super) fn tick(&mut self, room: u64) -> io::Result<Observation> {
        let mut out =
            Observation { state: "disabled by the memory room setting", requested: 0, swapped: 0, available: 0 };
        if self.standing && self.ranked.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
            rank_work(&self.group)?;
            self.ranked = Some(Instant::now());
        }
        if room == 0 {
            return Ok(out);
        }
        if self.unavailable.is_some() {
            out.state = "unavailable on this service; turn admission remains active";
            return Ok(out);
        }
        let memory = memory()?;
        out.available = memory.available;
        let stat = stat(&self.group)?;
        if let Some((swapped, refaults)) = self.last.take() {
            let returned = stat.refaults.saturating_sub(refaults).saturating_mul(self.page);
            if returned >= swapped / 4 {
                self.rest();
                out.state = "recently reclaimed pages are in use again; resting";
                return Ok(out);
            }
            self.rest = FIRST_REST;
        }
        if self.rest_until.is_some_and(|until| Instant::now() < until) {
            out.state = "resting after ineffective reclaim";
            return Ok(out);
        }
        // An explicit setting may exceed the machine's capacity; never try to
        // squeeze a service toward an impossible amount of free physical RAM.
        let target = room.saturating_add(ABOVE.min(memory.total / 10)).min(memory.total * 3 / 4);
        if memory.available >= target {
            out.state = "enough memory is available";
            return Ok(out);
        }
        if pressure(&self.group)? >= 10.0 || pressure(Path::new("/proc/pressure"))? >= 10.0 {
            out.state = "memory is already stalling; leaving the working set alone";
            return Ok(out);
        }
        let swap = swap_free(&self.group, memory.swap_free)?;
        if swap < LEAST {
            out.state = "no swap headroom in the machine or service";
            return Ok(out);
        }
        // A bounded probe lets refault feedback stop the next round before a
        // busy working set is repeatedly displaced. Active/inactive counters
        // do not identify cold anonymous pages consistently across both LRUs.
        let amount = (target - memory.available).min(STEP).min(stat.anon / 8).min(swap);
        if amount < LEAST {
            out.state = "too little anonymous memory to reclaim";
            return Ok(out);
        }
        let before = number(&self.group.join("memory.swap.current"))?;
        let asked = if self.plain { format!("{amount}") } else { format!("{amount} swappiness=max") };
        match fs::write(self.group.join("memory.reclaim"), asked) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        out.requested = amount;
        out.swapped = number(&self.group.join("memory.swap.current"))?.saturating_sub(before);
        out.available = memory_read("MemAvailable:")?;
        if out.swapped < LEAST {
            self.rest();
            out.state = "no useful swap increase; resting";
        } else {
            // Include refaults during the reclaim syscall in the next sample.
            self.last = Some((out.swapped, stat.refaults));
            out.state = "reclaiming anonymous pages";
        }
        Ok(out)
    }

    fn rest(&mut self) {
        self.rest_until = Some(Instant::now() + self.rest);
        self.rest = (self.rest * 2).min(MAX_REST);
    }
}

fn service_group() -> io::Result<PathBuf> {
    let text = fs::read_to_string("/proc/self/cgroup")?;
    let own = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::other("no unified cgroup hierarchy"))?;
    let own = Path::new(own);
    let service = if own.file_name().is_some_and(|name| name == "node") { own.parent() } else { Some(own) }
        .ok_or_else(|| io::Error::other("no service cgroup"))?;
    let name = service.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if !name.starts_with("fastctx") || !name.ends_with(".service") {
        return Err(io::Error::other("the node is not in its own fastctx service"));
    }
    Ok(Path::new("/sys/fs/cgroup").join(service.strip_prefix("/").map_err(io::Error::other)?))
}

struct Memory {
    total: u64,
    available: u64,
    swap_free: u64,
}
fn memory() -> io::Result<Memory> {
    let text = fs::read_to_string("/proc/meminfo")?;
    Ok(Memory {
        total: kib(&text, "MemTotal:")?,
        available: kib(&text, "MemAvailable:")?,
        swap_free: kib(&text, "SwapFree:")?,
    })
}
fn memory_read(name: &str) -> io::Result<u64> {
    kib(&fs::read_to_string("/proc/meminfo")?, name)
}
fn kib(text: &str, name: &str) -> io::Result<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .and_then(|kib| kib.checked_mul(1024))
        .ok_or_else(|| io::Error::other(format!("missing or invalid {name} in meminfo")))
}
fn number(path: &Path) -> io::Result<u64> {
    fs::read_to_string(path)?.trim().parse().map_err(io::Error::other)
}
struct Stat {
    anon: u64,
    refaults: u64,
}
fn stat(group: &Path) -> io::Result<Stat> {
    let text = fs::read_to_string(group.join("memory.stat"))?;
    let field = |name| {
        text.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(' ')?;
                (key == name).then(|| value.parse::<u64>().ok()).flatten()
            })
            .ok_or_else(|| io::Error::other(format!("missing {name} in memory.stat")))
    };
    Ok(Stat { anon: field("anon")?, refaults: field("workingset_refault_anon")? })
}
fn swap_free(group: &Path, system: u64) -> io::Result<u64> {
    let mut free = system;
    for ancestor in group.ancestors().take_while(|at| *at != Path::new("/sys/fs/cgroup")) {
        let text = fs::read_to_string(ancestor.join("memory.swap.max"))?;
        if text.trim() != "max" {
            let max: u64 = text.trim().parse().map_err(io::Error::other)?;
            free = free.min(max.saturating_sub(number(&ancestor.join("memory.swap.current"))?));
        }
    }
    Ok(free)
}
fn pressure(group: &Path) -> io::Result<f64> {
    let path = if group == Path::new("/proc/pressure") { group.join("memory") } else { group.join("memory.pressure") };
    let text = fs::read_to_string(path)?;
    text.lines()
        .find_map(|line| line.strip_prefix("full "))
        .and_then(|rest| rest.split_whitespace().find_map(|field| field.strip_prefix("avg10=")))
        .and_then(|number| number.parse().ok())
        .ok_or_else(|| io::Error::other("missing memory pressure"))
}
fn rank_work(group: &Path) -> io::Result<()> {
    let mut groups = vec![group.to_path_buf()];
    let me = std::process::id();
    while let Some(at) = groups.pop() {
        let text = match fs::read_to_string(at.join("cgroup.procs")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for pid in text.lines().filter_map(|pid| pid.parse::<u32>().ok()).filter(|pid| *pid != me) {
            let path = PathBuf::from(format!("/proc/{pid}/oom_score_adj"));
            if fs::read_to_string(&path)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .is_some_and(|score| score < 300)
            {
                // One gone meanwhile, or one this account may not rank (another
                // user's, or one that is not dumpable), is passed over: it
                // holds back neither the rest nor the reclaiming after them.
                match fs::write(path, "300") {
                    Ok(()) => {}
                    Err(error)
                        if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied)
                            || matches!(error.raw_os_error(), Some(1 | 3)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let entries = match fs::read_dir(&at) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                groups.push(entry.path());
            }
        }
    }
    Ok(())
}
