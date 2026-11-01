//! The worst-case soak (`specs/world-engine.md`, 验证的设施): a real node
//! held to a small server's two cores and two gigabytes, its World on a disk
//! of its own, under the load of many conversations, its interfaces and
//! phones for the drill's minutes, while its line is cut, dropped again and
//! again and frozen, its disk slowed and hung, a job runs away and calls
//! storm in, and a job on another machine steps back onto it every second.
//! Then its own readings, what its owner was told, what the hub logged of
//! its line and what its users saw are held to the foundation's numbers
//! (`specs/world-foundation.md`, 地基的数字).
//!
//! Run by `scripts/soak.py`, which gives it the disk, the slice, a model
//! mock and Codex: `soak_drill CONFIG.json`.

// A drill drives the hub from outside it, and is not held to the roles of
// the hub's threads (`clippy.toml`).
#![allow(clippy::disallowed_methods)]

mod clients;
mod faults;
mod judge;
#[path = "../network_drill/proxy.rs"]
#[allow(dead_code)]
mod proxy;
mod setup;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::json;

use clients::{Conversations, Dice, Opening, Seen};
use faults::{Faults, say};
use judge::{Alerts, Checks, Marks, Process};
use setup::Lab;

/// A cycle of what goes wrong; a drill longer than two of them runs them
/// an hour and more apart, as each kind of alert is told once an hour.
const CYCLE: Duration = Duration::from_secs(65 * 60);

#[tokio::main]
async fn main() -> Result<()> {
    anyhow::ensure!(cfg!(target_os = "linux"), "the soak runs on Linux, in a slice of its own");
    let config: setup::Config =
        serde_json::from_slice(&std::fs::read(std::env::args().nth(1).context("the drill's configuration")?)?)?;
    say(&format!("soak: seed {}, {} minutes", clients::seed(), config.minutes));
    let started = Instant::now();
    let lab = setup::build(config.clone()).await?;
    lab.soak.hold("0-1")?;
    let faults = Arc::new(Faults::default());
    let marks = Arc::new(Marks::default());
    lab.soak.watch(faults.clone(), marks.clone());

    let seen = Arc::new(Seen::default());
    let conversations = Arc::new(Conversations::default());
    let alerts = Arc::new(Alerts::default());
    let process = Arc::new(Process::default());
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let (all_stop, all_stopping) = tokio::sync::watch::channel(false);

    tokio::spawn(conversations.clone().follow(lab.local.clone(), seen.clone(), all_stopping.clone()));
    {
        let (alerts, owner, stopping) = (alerts.clone(), lab.owner.clone(), all_stopping.clone());
        tokio::spawn(async move { alerts.follow(owner, stopping).await });
    }
    let open = Opening { agent: lab.agent.clone(), home: config.codex_home.clone() };

    let seeded = seed(&lab, &seen, &conversations, &open).await;
    lab.soak.disk(config.iops)?;
    let restart = match faults::restart(&lab, &faults).await {
        Ok(took) => {
            say(&format!("soak: the node took requests again {:.2} s after it was ended", took.as_secs_f64()));
            Some(took)
        }
        Err(error) => {
            say(&format!("soak: {error:#}"));
            None
        }
    };

    mark(&lab, &marks, "load").await;
    let load = Instant::now();
    let length = Duration::from_secs(config.minutes * 60);
    tokio::spawn(sample(process.clone(), lab.soak.pids(), all_stopping.clone()));

    // Not started is told among the checks; the rest of the load goes on.
    let patrol = match faults::Patrol::start(&lab, &seen, &open, &conversations).await {
        Ok(patrol) => Some(patrol),
        Err(error) => {
            say(&format!("soak: the patrol did not start: {error:#}"));
            None
        }
    };
    let runaway = match open.open_soon(&lab.local, &seen, 1).await {
        Ok(runaway) => {
            conversations.rest(&runaway, Duration::from_secs(120)).await;
            Some(runaway)
        }
        Err(said) => {
            say(&format!("soak: the runaway's conversation did not open: {said}"));
            None
        }
    };
    let mut users = tokio::task::JoinSet::new();
    for index in 0..config.workers as u64 {
        users.spawn(clients::person(
            index,
            lab.local.clone(),
            conversations.clone(),
            seen.clone(),
            open.clone(),
            stopping.clone(),
        ));
    }
    for index in 0..config.watchers as u64 {
        users.spawn(clients::interface(
            index,
            lab.local.clone(),
            conversations.clone(),
            seen.clone(),
            stopping.clone(),
        ));
    }
    for (index, (directory, owner)) in lab.phones.iter().enumerate() {
        users.spawn(clients::phone(
            index as u64,
            directory.clone(),
            owner.clone(),
            conversations.clone(),
            seen.clone(),
            open.clone(),
            stopping.clone(),
        ));
    }

    let cycles = if length >= CYCLE * 2 { (length.as_secs() / CYCLE.as_secs()).max(1) } else { 1 };
    let each = if cycles > 1 { CYCLE } else { length };
    for cycle in 0..cycles {
        let began = load + each * cycle as u32;
        let end = (began + each).min(load + length);
        run_cycle(&lab, &seen, &conversations, &faults, runaway.as_ref(), &marks, cycle, began, end).await;
    }
    tokio::time::sleep_until((load + length).into()).await;

    say("soak: the load stops");
    let _ = stop.send(true);
    let patrol_seconds = patrol.as_ref().map_or(0, |patrol| patrol.began.elapsed().as_secs());
    let patrol_steps = match &patrol {
        Some(patrol) => match patrol.finish(&lab.local).await {
            Ok(steps) => Some(steps),
            Err(error) => {
                say(&format!("soak: {error}"));
                None
            }
        },
        None => None,
    };
    let _ = tokio::time::timeout(clients::OUTCOME + Duration::from_secs(30), async {
        while users.join_next().await.is_some() {}
    })
    .await;
    users.abort_all();
    mark(&lab, &marks, "end").await;

    // At rest: nobody watching or speaking, no job running.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let idle_cpu = match lab.soak.pid() {
        Some(pid) => {
            let before = judge::cpu(pid);
            tokio::time::sleep(Duration::from_secs(60)).await;
            before.zip(judge::cpu(pid)).map(|(before, after)| (after - before).as_secs_f64() / 60.0)
        }
        None => None,
    };
    let scenes = judge::readings(lab.soak.root()).await.map(|(_, _, scenes)| scenes).unwrap_or_default();
    let _ = all_stop.send(true);

    let marks = marks.lock().expect("never poisoned").clone();
    let evidence = judge::Evidence {
        marks: &marks,
        faults: &faults.all(),
        samples: &seen.samples(),
        hung: seen.hung(),
        failed: seen.failed(),
        alerts: &alerts,
        process: &process,
        hub_log: lab.hub.log(),
        node_log: faults::node_log(lab.soak.root()),
        scenes,
        restart,
        idle_cpu,
        patrol: patrol_steps,
        patrol_seconds,
        held: lab.soak.held(),
        ended: lab.soak.ended(),
        iops: config.iops,
        strays: strays(&config),
        seeded: (seeded, config.conversations, config.seeding),
    };
    let mut checks = Checks::default();
    judge::judge(&evidence, &mut checks);
    let result = json!({
        "seed": clients::seed(),
        "minutes": config.minutes,
        "conversations": conversations.count(),
        "passed": checks.passed,
        "failed": checks.failed,
        "checks": checks.lines,
        "faults": faults.all(),
        "watches": {"opened": seen.watches_opened.load(std::sync::atomic::Ordering::Relaxed), "broken": seen.watches_broken.load(std::sync::atomic::Ordering::Relaxed)},
        "readings": marks.iter().map(|mark| json!({"name": mark.name, "pid": mark.pid, "readings": mark.readings})).collect::<Vec<_>>(),
    });
    std::fs::write(config.base.join("soak-result.json"), serde_json::to_vec_pretty(&result)?)?;
    say(&format!(
        "SOAK: {} checks passed, {} failed, in {} minutes",
        checks.passed,
        checks.failed,
        started.elapsed().as_secs() / 60
    ));
    lab.node_line.end().await.ok();
    lab.device_line.end().await.ok();
    drop(lab);
    anyhow::ensure!(checks.failed == 0, "{} soak checks failed", checks.failed);
    Ok(())
}

/// Opens the conversations the node is to hold before the load, and gives
/// each its turns, a few at a time, for at most the minutes they are given
/// in: how many it took.
async fn seed(lab: &Lab, seen: &Arc<Seen>, conversations: &Arc<Conversations>, open: &Opening) -> usize {
    let began = Instant::now();
    let at_once = Arc::new(tokio::sync::Semaphore::new(12));
    let mut seeding = tokio::task::JoinSet::new();
    for index in 0..lab.config.conversations as u64 {
        let (local, seen, conversations, open, at_once) =
            (lab.local.clone(), seen.clone(), conversations.clone(), open.clone(), at_once.clone());
        let turns = lab.config.turns;
        seeding.spawn(async move {
            let _turn = at_once.acquire_owned().await;
            let resident = open.open(&local, &seen, "seed", 10_000 + index).await?;
            conversations.rest(&resident, clients::OUTCOME).await;
            for turn in 0..turns {
                let text = format!("soak seed {index}.{turn}: tell me about this machine and what it keeps");
                clients::type_and_send(&local, &seen, &["seed", "seed"], &resident, None, &text).await;
                conversations.rest(&resident, clients::OUTCOME).await;
            }
            conversations.add(resident);
            Some(())
        });
    }
    let mut opened = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(lab.config.seeding * 60);
    loop {
        match tokio::time::timeout_at(deadline, seeding.join_next()).await {
            Ok(Some(done)) => opened += usize::from(matches!(done, Ok(Some(())))),
            Ok(None) => break,
            Err(_) => {
                seeding.abort_all();
                break;
            }
        }
    }
    say(&format!("soak: {opened} conversations seeded in {} s", began.elapsed().as_secs()));
    opened
}

/// Worlds in the users' homes other than the node's own: one is what a
/// tool of the World makes when it finds no World where it looks.
fn strays(config: &setup::Config) -> Vec<String> {
    [config.home.clone(), config.base.join("user")]
        .iter()
        .filter_map(|home| std::fs::read_dir(home).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with(".fastctx"))
                && *path != config.world
        })
        .map(|path| path.display().to_string())
        .collect()
}

/// The node's readings now, kept under `name`.
async fn mark(lab: &Lab, marks: &Marks, name: &str) {
    judge::mark(lab.soak.root(), &lab.soak.pids(), marks, name).await;
}

/// The node's resident memory every 15 s, until `stop`, of whichever
/// process it has.
async fn sample(process: Arc<Process>, pid: Arc<AtomicU32>, mut stop: tokio::sync::watch::Receiver<bool>) {
    while !*stop.borrow() {
        process.note(Some(pid.load(Ordering::Acquire)).filter(|pid| *pid != 0));
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(15)) => {},
            _ = stop.changed() => {},
        }
    }
}

/// Sleeps until `offset` into a cycle that began at `began`, unless that is
/// past `end`; whether there is still time.
async fn until(began: Instant, offset: Duration, end: Instant) -> bool {
    let at = began + offset;
    if at >= end {
        return false;
    }
    tokio::time::sleep_until(at.into()).await;
    true
}

/// One cycle of what goes wrong.
#[allow(clippy::too_many_arguments)]
async fn run_cycle(
    lab: &Lab,
    seen: &Arc<Seen>,
    conversations: &Arc<Conversations>,
    faults: &Faults,
    runaway: Option<&fastctx_types::ResidentId>,
    marks: &Marks,
    cycle: u64,
    began: Instant,
    end: Instant,
) {
    let minutes = |m: u64| Duration::from_secs(m * 60);
    // A fault not made is said and judged; the cycle goes on.
    let told = |result: Result<()>| {
        if let Err(error) = result {
            say(&format!("soak: {error:#}"));
        }
    };
    // Connected again four times within a minute, an hour after the last
    // time the owner was told of it; later cycles have their cuts anyway.
    if cycle == 0 && until(began, minutes(2), end).await {
        faults::drops(lab, faults).await;
    }
    if let Some(runaway) = runaway
        && until(began, minutes(4), end).await
    {
        told(faults::runaway(&lab.local, seen, runaway, faults, minutes(3)).await);
    }
    if until(began, minutes(8), end).await {
        faults::storm(lab, conversations, seen.clone(), faults).await;
    }
    if until(began, Duration::from_secs(9 * 60 + 30), end).await {
        faults::phones_cut(lab, faults);
    }
    if until(began, minutes(10), end).await {
        mark(lab, marks, &format!("slow-{cycle}")).await;
        told(faults::disk(lab, faults, "slow", 25, Duration::from_secs(60)).await);
        tokio::time::sleep(Duration::from_secs(10)).await;
        mark(lab, marks, &format!("slowed-{cycle}")).await;
    }
    if until(began, minutes(12), end).await {
        mark(lab, marks, &format!("hang-{cycle}")).await;
        told(faults::disk(lab, faults, "hang", 1, Duration::from_secs(160)).await);
        tokio::time::sleep(Duration::from_secs(20)).await;
        mark(lab, marks, &format!("hung-{cycle}")).await;
    }
    if until(began, minutes(16), end).await {
        faults::frozen(lab, faults, Duration::from_secs(75)).await;
    }
    let mut dice = Dice::new(clients::seed() ^ cycle);
    let mut at = minutes(19);
    while until(began, at, end).await {
        if dice.below(3) == 0 {
            faults::phones_cut(lab, faults);
        } else {
            faults::cut(lab, faults);
        }
        at += dice.seconds(120, 240);
    }
}
