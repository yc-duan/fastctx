//! What the soak measured, held to the foundation's numbers
//! (`specs/world-foundation.md`, 地基的数字): from the node's own readings,
//! what its owner was told, what the hub logged of its line, and what its
//! users saw.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use fastctx_client::World;
use fastctx_types::health::Health;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::readings::{Readings, Timing};
use fastctx_types::trouble::TroubleKind;
use fastctx_types::{Call, Reply};
use serde_json::{Value, json};

use crate::clients::Sample;
use crate::faults::{Fault, say};

/// What was checked, and how it came out.
#[derive(Default)]
pub(crate) struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub lines: Vec<Value>,
}

impl Checks {
    pub fn check(&mut self, what: &str, ok: bool, detail: Value) {
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        let shown: String = detail.to_string().chars().take(1500).collect();
        println!("{} {what} {shown}", if ok { "PASS" } else { "FAIL" });
        self.lines.push(json!({"what": what, "ok": ok, "detail": detail}));
    }
}

/// The node's readings, its health and the scenes it kept, asked through
/// its own door.
pub(crate) async fn readings(root: &Path) -> Option<(Readings, Health, Vec<String>)> {
    let world = World::new(WorldDir::under(root), Peer::Command { resident: None, session: None });
    match tokio::time::timeout(Duration::from_secs(30), world.call(Call::DoctorReadings)).await {
        Ok(Ok((Reply::DoctorReadings { readings, health, scenes }, _))) => Some((*readings, health, scenes)),
        _ => None,
    }
}

/// The node's readings at one moment of the drill, and the process that
/// gave them: each counts from its own start.
#[derive(Clone)]
pub(crate) struct Mark {
    pub name: String,
    pub readings: Readings,
    pub pid: u32,
    pub at: Instant,
}

/// The marks taken, in the order they were, and those the node did not
/// answer for.
#[derive(Default)]
pub(crate) struct Marks {
    taken: Mutex<Vec<Mark>>,
    missed: Mutex<Vec<String>>,
}

impl Marks {
    pub fn taken(&self) -> Vec<Mark> {
        self.taken.lock().expect("never poisoned").clone()
    }

    pub fn missed(&self) -> Vec<String> {
        self.missed.lock().expect("never poisoned").clone()
    }
}

/// Takes the readings of the node at `root` as `name`, while its process
/// stays `pid`'s: missed when it changed meanwhile, or the node did not
/// answer.
pub(crate) async fn mark(root: &Path, pid: &AtomicU32, marks: &Marks, name: &str) {
    let before = pid.load(Ordering::Acquire);
    match readings(root).await {
        Some((readings, _, _)) if before != 0 && pid.load(Ordering::Acquire) == before => {
            let mark = Mark { name: name.to_string(), readings, pid: before, at: Instant::now() };
            marks.taken.lock().expect("never poisoned").push(mark);
        }
        _ => {
            say(&format!("soak: the node's readings could not be taken at {name}"));
            marks.missed.lock().expect("never poisoned").push(name.to_string());
        }
    }
}

/// Two stretches of one process's readings as one: counts and timings
/// added, levels as of `b`.
fn joined(a: &Readings, b: &Readings) -> Readings {
    let mut joined = b.clone();
    for count in &mut joined.counts {
        count.value += a.count(&count.name).unwrap_or(0);
    }
    for timing in &mut joined.timings {
        let Some(other) = a.timing(&timing.name) else { continue };
        if other.buckets.len() == timing.buckets.len() {
            for (bucket, more) in timing.buckets.iter_mut().zip(&other.buckets) {
                *bucket += more;
            }
        }
        timing.count += other.count;
        timing.sum_us += other.sum_us;
        timing.max_us = timing.max_us.max(other.max_us);
        *timing = timing.since(None, &joined.bounds_us);
    }
    joined.violations_total += a.violations_total;
    joined
}

/// The readings between each two marks of one process in a row, from the
/// load's mark on, joined, of the stretches `counted` keeps. What a process
/// that ended on its own did after its last mark went with it, and a mark
/// the node did not answer for joins the stretches either side of it.
fn stretches(marks: &[Mark], counted: impl Fn(&Mark, &Mark) -> bool) -> Option<Readings> {
    let load = marks.iter().position(|mark| mark.name == "load")?;
    let mut total: Option<Readings> = None;
    for pair in marks[load..].windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if from.pid != to.pid || !counted(from, to) {
            continue;
        }
        let stretch = to.readings.since(&from.readings);
        total = Some(match total {
            Some(total) => joined(&total, &stretch),
            None => stretch,
        });
    }
    total
}

/// The stretches the disk was slowed past a small server's or hung, and the
/// moments after while what waited on it catches up.
fn rough(faults: &[Fault]) -> Vec<(Instant, Instant)> {
    faults
        .iter()
        .filter(|fault| matches!(fault.kind, "slow" | "hang"))
        .map(|fault| (fault.began, fault.ended.unwrap_or(fault.began) + Duration::from_secs(20)))
        .collect()
}

fn timing<'a>(readings: &'a Readings, name: &str) -> Option<&'a Timing> {
    readings.timing(name)
}

fn shown(timing: Option<&Timing>) -> Value {
    timing.map_or(json!(null), |timing| {
        json!({"count": timing.count, "p50_us": timing.p50_us, "p99_us": timing.p99_us, "max_us": timing.max_us})
    })
}

/// An alert as the owner's phone was told it: its kind, the machine it is
/// of, when the drill first saw it and when the hub raised it.
type Told = (TroubleKind, Option<String>, Instant, jiff::Timestamp);

/// The owner's alerts as they arrived, by id.
#[derive(Default)]
pub(crate) struct Alerts(Mutex<BTreeMap<u64, Told>>);

impl Alerts {
    /// Watches the owner's alerts until `stop`, as their phone is told.
    pub async fn follow(&self, owner: World, mut stop: tokio::sync::watch::Receiver<bool>) {
        while !*stop.borrow() {
            if let Ok(Ok(accounts)) = tokio::time::timeout(Duration::from_secs(10), owner.accounts()).await {
                let mut seen = self.0.lock().expect("never poisoned");
                for trouble in accounts.hubs.into_iter().flat_map(|hub| hub.troubles) {
                    seen.entry(trouble.id).or_insert_with(|| {
                        say(&format!("alert: {:?} of {:?}", trouble.kind, trouble.machine));
                        (trouble.kind, trouble.machine.map(|machine| machine.to_string()), Instant::now(), trouble.at)
                    });
                }
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(1)) => {},
                _ = stop.changed() => {},
            }
        }
    }

    /// The first alert of `kind` about `machine` seen after `since`.
    fn first(&self, kind: TroubleKind, machine: &str, since: Instant) -> Option<Instant> {
        self.0
            .lock()
            .expect("never poisoned")
            .values()
            .filter(|(seen_kind, about, at, _)| *seen_kind == kind && about.as_deref() == Some(machine) && *at >= since)
            .map(|(_, _, at, _)| *at)
            .min()
    }

    /// How long before `at` an alert of `kind` about `machine` was last
    /// seen, within the hour: one then keeps the hub from telling it again.
    fn quieting(&self, kind: TroubleKind, machine: &str, at: Instant) -> Option<f64> {
        self.0
            .lock()
            .expect("never poisoned")
            .values()
            .filter(|(seen_kind, about, seen, _)| *seen_kind == kind && about.as_deref() == Some(machine) && *seen < at)
            .map(|(_, _, seen, _)| at.duration_since(*seen))
            .filter(|before| *before < Duration::from_secs(3600))
            .min()
            .map(|before| before.as_secs_f64())
    }

    fn all(&self) -> Value {
        json!(self
            .0
            .lock()
            .expect("never poisoned")
            .iter()
            .map(|(id, (kind, machine, _, at))| json!({"id": id, "kind": format!("{kind:?}"), "machine": machine, "at": at.to_string()}))
            .collect::<Vec<_>>())
    }
}

/// The node's resident memory and CPU, from outside, over the drill.
#[derive(Default)]
pub(crate) struct Process(Mutex<Vec<(Instant, u64)>>);

impl Process {
    pub fn note(&self, pid: Option<u32>) {
        if let Some(rss) = pid.and_then(resident) {
            self.0.lock().expect("never poisoned").push((Instant::now(), rss));
        }
    }

    fn samples(&self) -> Vec<(Instant, u64)> {
        self.0.lock().expect("never poisoned").clone()
    }
}

/// A process's resident bytes, as the kernel counts them.
fn resident(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kib| kib * 1024)
}

/// A process's CPU time so far, counted in the hundredths of a second
/// Linux tells it in.
pub(crate) fn cpu(pid: u32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

/// The times `machine` connected to the hub and left it, from its log.
fn line_events(hub_log: &str, machine: &str) -> Vec<(jiff::Timestamp, String)> {
    let connected = format!("{machine} connected");
    let disconnected = format!("{machine} disconnected");
    hub_log
        .lines()
        .filter_map(|line| {
            let (at, text) = line.split_once(' ')?;
            let at: jiff::Timestamp = at.parse().ok()?;
            (text.starts_with(&connected) || text.starts_with(&disconnected)).then(|| (at, text.to_string()))
        })
        .collect()
}

/// Everything the drill saw, to judge.
pub(crate) struct Evidence<'a> {
    pub marks: &'a [Mark],
    /// The marks the node did not answer for.
    pub missed: Vec<String>,
    pub faults: &'a [Fault],
    pub samples: &'a [Sample],
    pub hung: BTreeMap<&'static str, u64>,
    pub failed: BTreeMap<&'static str, (u64, String)>,
    pub alerts: &'a Alerts,
    pub process: &'a Process,
    pub hub_log: String,
    pub node_log: String,
    pub scenes: Vec<String>,
    /// How long the node took to take requests again once ended at once;
    /// none when it did not within a minute.
    pub restart: Option<Duration>,
    pub idle_cpu: Option<f64>,
    pub patrol: Option<crate::faults::Steps>,
    pub patrol_seconds: u64,
    /// What the node's slice held it to, and what it came to.
    pub held: Value,
    /// The times the node ended on its own and was started again.
    pub ended: Vec<crate::setup::Ended>,
    pub iops: u64,
    /// Worlds found that the drill did not make.
    pub strays: Vec<String>,
    /// The conversations opened and spoken to before the load, of those
    /// asked, in the minutes given.
    pub seeded: (usize, usize, u64),
}

/// Holds what was seen to the foundation's numbers.
pub(crate) fn judge(evidence: &Evidence<'_>, checks: &mut Checks) {
    let whole = stretches(evidence.marks, |_, _| true);
    // The disk slowed past a small server's, or hung, is not the reference
    // machine: latencies are held to their numbers outside those stretches;
    // everything else, all the way through.
    let rough = rough(evidence.faults);
    let calm =
        stretches(evidence.marks, |from, to| !rough.iter().any(|(began, ended)| from.at < *ended && to.at > *began));
    checks.check(
        "the node answered for its readings at every mark",
        whole.is_some() && evidence.missed.is_empty(),
        json!({"taken": evidence.marks.iter().map(|mark| mark.name.clone()).collect::<Vec<_>>(), "missed": evidence.missed}),
    );
    if let Some(whole) = &whole {
        judge_readings(whole, calm.as_ref().unwrap_or(whole), checks);
    }

    let (seeded, asked, minutes) = evidence.seeded;
    checks.check(
        "the node took nine in ten of the conversations given it before the load, in the minutes they were given",
        seeded * 10 >= asked * 9,
        json!({"seeded": seeded, "asked": asked, "minutes": minutes}),
    );
    checks.check(
        "nothing of the World's that the node's host started found another World, or made one",
        evidence.strays.is_empty(),
        json!(evidence.strays),
    );
    let held = &evidence.held;
    let io = held["io_max"].as_str().unwrap_or_default();
    checks.check(
        "the node ran held to two cores, two gigabytes and a small server's disk, and was never killed for memory",
        held["cpus"] == "0-1"
            && held["memory_max"] == "2147483648"
            && io.contains(&format!("riops={}", evidence.iops))
            && io.contains(&format!("wiops={}", evidence.iops))
            && held["memory_events"].as_str().is_some_and(|events| events.contains("oom_kill 0")),
        held.clone(),
    );
    checks.check(
        "the node never ended on its own, for its service to start it again",
        evidence.ended.is_empty(),
        json!(evidence.ended),
    );
    let samples = evidence.process.samples();
    let peak = samples.iter().map(|(_, rss)| *rss).max().unwrap_or(0);
    let quarter = samples.len() / 4;
    let median = |part: &[(Instant, u64)]| {
        let mut values: Vec<u64> = part.iter().map(|(_, rss)| *rss).collect();
        values.sort_unstable();
        values.get(values.len() / 2).copied().unwrap_or(0)
    };
    let (early, late) = if quarter > 0 {
        (median(&samples[quarter..quarter * 2]), median(&samples[samples.len() - quarter..]))
    } else {
        (0, 0)
    };
    checks.check(
        "the node kept at most 300 MB resident, and no more late in the drill than early on",
        peak > 0 && peak <= 300 * 1024 * 1024 && late <= early + early / 10,
        json!({"peak_mib": peak / 1024 / 1024, "second_quarter_mib": early / 1024 / 1024, "last_quarter_mib": late / 1024 / 1024, "samples": samples.len()}),
    );
    checks.check(
        "the node took requests again within 5 s of being ended at once, with its World full",
        evidence.restart.is_some_and(|took| took <= Duration::from_secs(5)),
        json!({"seconds": evidence.restart.map(|took| took.as_secs_f64())}),
    );
    checks.check(
        "at rest, the node used next to no CPU, however much its World holds",
        evidence.idle_cpu.is_some_and(|share| share <= 0.03),
        json!({"share_of_a_core": evidence.idle_cpu}),
    );

    let hung: u64 = evidence.hung.values().sum();
    checks.check(
        "every operation came to its outcome within two minutes",
        hung == 0,
        json!({"without_outcome": evidence.hung, "failed": evidence.failed}),
    );
    let ran: BTreeMap<&str, usize> = evidence.samples.iter().fold(BTreeMap::new(), |mut ran, sample| {
        *ran.entry(sample.kind).or_default() += 1;
        ran
    });
    let made: std::collections::BTreeSet<&str> = evidence.faults.iter().map(|fault| fault.kind).collect();
    checks.check(
        "every fault meant was made: drops, a runaway job, a storm, a slow disk and a hung one, a frozen line",
        ["drops", "runaway", "storm", "slow", "hang", "frozen"].iter().all(|kind| made.contains(kind)),
        json!(made),
    );
    checks.check(
        "the intended load ran: typing, sending, stopping and opening here and from phones, and the storm",
        ["type", "send", "stop", "open", "phone.type", "phone.send", "storm", "storm.phone", "watch"]
            .iter()
            .all(|kind| ran.get(kind).copied().unwrap_or(0) > 0),
        json!(ran),
    );
    // A step a second and the step's own time: hundreds in half an hour.
    checks.check(
        "every step the patrol took on another machine came back to its conversation, none lost",
        evidence.patrol.is_some_and(|steps| steps.taken >= evidence.patrol_seconds / 4 && steps.told == steps.taken),
        json!({"steps": evidence.patrol, "seconds": evidence.patrol_seconds}),
    );

    judge_clients(evidence, checks);
    judge_line(evidence, checks);
    judge_alerts(evidence, checks);
}

/// What the node's own readings say, over the load (`whole`) and outside
/// the stretches its disk was slowed or hung (`calm`).
fn judge_readings(whole: &Readings, calm: &Readings, checks: &mut Checks) {
    checks.check(
        "no violation of the threads' rules all through",
        whole.violations_total == 0,
        json!({"total": whole.violations_total, "violations": whole.violations}),
    );
    for runtime in ["node", "link", "data"] {
        let name = format!("runtime.{runtime}.waited");
        let waited = timing(whole, &name);
        checks.check(
            &format!("the {runtime} runtime's tasks waited for a thread at most 10 ms at the 99th percentile"),
            waited.is_some_and(|waited| waited.count > 0 && waited.p99_us <= 10_000),
            shown(waited),
        );
    }
    let poll = timing(whole, "tasks.poll");
    checks.check(
        "a task kept its thread at most 1 ms at the 99th percentile",
        poll.is_some_and(|poll| poll.count > 0 && poll.p99_us <= 1_000),
        shown(poll),
    );
    let beats = timing(whole, "link.heartbeat");
    let pongs = timing(whole, "link.pong");
    checks.check(
        "the heartbeats went and came back within 12 s of each other all through",
        beats.is_some_and(|beats| beats.count > 0)
            && pongs.is_some_and(|pongs| pongs.count > 0)
            && whole.count("link.heartbeat.late").unwrap_or(0) == 0
            && whole.count("link.pong.late").unwrap_or(0) == 0,
        json!({"heartbeat": shown(beats), "pong": shown(pongs), "late": whole.count("link.heartbeat.late"), "pong_late": whole.count("link.pong.late")}),
    );
    // A reading not there was never made: the node holds these all through.
    let shared = timing(whole, "locks.shared");
    checks.check(
        "locks over many units held at most 1 ms at the 99th percentile and 10 ms at the longest",
        shared.is_some_and(|shared| shared.count > 0 && shared.p99_us <= 1_000 && shared.max_us <= 10_000),
        json!({"shared": shown(shared), "locks": whole.timings.iter().filter(|timing| timing.name.starts_with("lock.")).map(|timing| json!({"name": timing.name, "p99_us": timing.p99_us, "max_us": timing.max_us, "count": timing.count})).collect::<Vec<_>>()}),
    );
    // A database's measures go by its file's stem: `storage.node.*` for
    // `node.sqlite3`. The node writes its own all through; its ledger, when
    // anything was spent.
    for (store, needed) in [("node", true), ("book", false)] {
        let operations = calm.count(&format!("storage.{store}.operations")).unwrap_or(0);
        let over = calm.count(&format!("storage.{store}.over_budget")).unwrap_or(0);
        let slow = calm.count(&format!("storage.{store}.slow")).unwrap_or(0);
        checks.check(
            &format!(
                "{store}.sqlite3: writes over 2 ms at most one in a hundred, none over 20 ms, on a small server's disk"
            ),
            (!needed || operations > 0) && over * 100 <= operations && slow == 0,
            json!({"operations": operations, "over_2ms": over, "over_20ms": slow, "write": shown(timing(calm, &format!("storage.{store}.write")))}),
        );
    }
    // Typing and sending go through the shared input, here and from phones;
    // stopping, here; words said straight in are held to the same when any
    // were.
    for (kind, here, phones) in [("composer", true, true), ("stop", true, false), ("talk", false, false)] {
        let local = timing(calm, &format!("call.{kind}"));
        let remote = timing(calm, &format!("remote.{kind}"));
        let held = |timing: Option<&Timing>, needed: bool| {
            timing.map_or(!needed, |timing| (!needed || timing.count > 0) && timing.p99_us <= 100_000)
        };
        checks.check(
            &format!(
                "{kind}, from the node's door to its answer, here and from phones: within 100 ms at the 99th percentile"
            ),
            held(local, here) && held(remote, phones),
            json!({"here": shown(local), "phones": shown(remote)}),
        );
    }
    // Opening starts the host's own thread, which is not the node's to answer for.
    println!(
        "OPENING {}",
        json!({"here": shown(timing(calm, "call.open")), "phones": shown(timing(calm, "remote.open"))})
    );
    // The databases' reads, their waits for a connection and their commits,
    // all through: told, for what the rest is judged on to be read by.
    println!(
        "STORAGE {}",
        json!(["node", "book"].map(|store| json!({
            "store": store,
            "read": shown(timing(whole, &format!("storage.{store}.read"))),
            "read_wait": shown(timing(whole, &format!("storage.{store}.read_wait"))),
            "commit": shown(timing(whole, &format!("storage.{store}.commit"))),
        })))
    );
}

/// What the users saw of each kind of operation, outside the stretches the
/// disk was slowed or hung: told as it is, the node's own numbers being
/// what is held to the foundation's.
fn judge_clients(evidence: &Evidence<'_>, checks: &mut Checks) {
    let rough = rough(evidence.faults);
    let mut kinds: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    for sample in evidence.samples {
        if sample.ok && !rough.iter().any(|(from, to)| sample.at >= *from && sample.at <= *to) {
            kinds.entry(sample.kind).or_default().push(sample.took);
        }
    }
    let told: BTreeMap<&str, Value> = kinds
        .iter_mut()
        .map(|(kind, took)| {
            took.sort_unstable();
            let at = |share: f64| took[((took.len() as f64 * share).ceil() as usize).clamp(1, took.len()) - 1];
            let ms = |duration: Duration| (duration.as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
            (*kind, json!({"count": took.len(), "p50_ms": ms(at(0.5)), "p99_ms": ms(at(0.99)), "max_ms": ms(at(1.0))}))
        })
        .collect();
    println!("CLIENTS {}", json!(told));
    let failed: u64 = evidence.failed.values().map(|(count, _)| count).sum();
    let all = evidence.samples.len() as u64;
    checks.check(
        "what the users asked mostly went through: at most one operation in twenty failed, busy and faults included",
        all > 0 && failed * 20 <= all,
        json!({"operations": all, "failed": failed, "by_kind": told}),
    );
}

/// No judgment of either end on the line other than what the drill did to
/// it: each connection after the first follows a cut, a freeze or the
/// node's end, by the drill's hand or its own (judged on its own).
fn judge_line(evidence: &Evidence<'_>, checks: &mut Checks) {
    let events = line_events(&evidence.hub_log, crate::setup::SOAK);
    let margin = jiff::SignedDuration::from_secs(45);
    let caused = |at: jiff::Timestamp| {
        evidence.faults.iter().any(|fault| {
            matches!(fault.kind, "cut" | "drops" | "frozen" | "restart" | "ended")
                && at >= fault.from
                && at <= fault.to.unwrap_or(fault.from) + margin
        })
    };
    let uncaused: Vec<&(jiff::Timestamp, String)> =
        events.iter().filter(|(at, text)| text.contains("disconnected") && !caused(*at)).collect();
    checks.check(
        "the hub and the node never took each other for gone but when the line was cut, frozen or the node ended",
        uncaused.is_empty()
            && events.iter().any(|(_, text)| text.contains("connected from") || text.contains("connected through")),
        json!({"uncaused": uncaused, "connections": events.len()}),
    );
    let woke = evidence.node_log.lines().filter(|line| line.contains("the machine resumed")).count();
    let silent = evidence
        .node_log
        .lines()
        .filter(|line| line.contains("nothing came from the hub"))
        .filter(|line| {
            line.split_once(' ').and_then(|(at, _)| at.parse::<jiff::Timestamp>().ok()).is_none_or(|at| !caused(at))
        })
        .count();
    checks.check(
        "the node never heard silence from the hub it was not made to, nor took its own busyness for a sleep",
        woke == 0 && silent == 0,
        json!({"woke": woke, "silent": silent}),
    );
}

/// Each thing made to go wrong reached the owner within a minute.
fn judge_alerts(evidence: &Evidence<'_>, checks: &mut Checks) {
    let soak = crate::setup::SOAK;
    let of = |kind: &'static str| evidence.faults.iter().filter(move |fault| fault.kind == kind);
    // Each kind of alert of one machine is told once an hour: the drill
    // makes each again only an hour after the last.
    if let Some(drops) = of("drops").next() {
        let ended = drops.ended.unwrap_or(drops.began);
        let seen = evidence.alerts.first(TroubleKind::Drops, soak, drops.began);
        checks.check(
            "connected again more than three times within the hour: the owner was told within a minute",
            seen.is_some_and(|seen| seen <= ended + Duration::from_secs(60)),
            json!({
                "after_seconds": seen.map(|seen| seen.duration_since(drops.began).as_secs_f64()),
                "kept_quiet_by_one_seconds_before": evidence.alerts.quieting(TroubleKind::Drops, soak, drops.began),
            }),
        );
    }
    for frozen in of("frozen") {
        let seen = evidence.alerts.first(TroubleKind::Silent, soak, frozen.began);
        // The hub takes the line for gone after 30 s of silence.
        checks.check(
            "a server gone silent: the owner was told within a minute of the hub taking it for gone",
            seen.is_some_and(|seen| seen <= frozen.began + Duration::from_secs(30 + 60)),
            json!({
                "after_seconds": seen.map(|seen| seen.duration_since(frozen.began).as_secs_f64()),
                "kept_quiet_by_one_seconds_before": evidence.alerts.quieting(TroubleKind::Silent, soak, frozen.began),
            }),
        );
    }
    for hang in of("hang") {
        let seen = evidence.alerts.first(TroubleKind::Stuck, soak, hang.began);
        // The scene names the moment the node knew it was stuck.
        let knew = evidence
            .scenes
            .iter()
            .filter_map(|scene| {
                let name = Path::new(scene).file_name()?.to_str()?;
                let millis: i64 = name.strip_prefix("scene-")?.strip_suffix(".txt")?.parse().ok()?;
                jiff::Timestamp::from_millisecond(millis).ok()
            })
            .find(|at| *at >= hang.from && hang.to.is_none_or(|to| *at <= to));
        let knew_after = knew.map(|knew| knew.duration_since(hang.from).as_secs_f64());
        checks.check(
            "a disk that hung: the node knew it was stuck and kept the scene",
            knew_after.is_some_and(|after| after <= 50.0),
            json!({"after_seconds": knew_after, "scenes": evidence.scenes.len()}),
        );
        let in_time = match (seen, knew_after) {
            (Some(seen), Some(knew_after)) => {
                Instant::duration_since(&seen, hang.began).as_secs_f64() <= knew_after + 60.0 + 60.0
            }
            _ => false,
        };
        checks.check(
            "stuck for a minute: the owner was told within the next minute",
            in_time,
            json!({
                "after_seconds": seen.map(|seen| seen.duration_since(hang.began).as_secs_f64()),
                "kept_quiet_by_one_seconds_before": evidence.alerts.quieting(TroubleKind::Stuck, soak, hang.began),
            }),
        );
    }
    println!("ALERTS {}", evidence.alerts.all());
}
