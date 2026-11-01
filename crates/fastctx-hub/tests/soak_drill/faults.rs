//! What goes wrong while the node soaks, each kept with when it began and
//! ended so its readings and alerts can be judged against it: its line cut
//! and cut again within the hour, frozen until the hub hears nothing, its
//! disk slowed and hung, a job running away, calls storming in. And the
//! patrol: a job on another machine stepping back onto this one every
//! second for as long as the drill runs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_client::World;
use fastctx_types::ResidentId;
use fastctx_types::run_facts::RunFacts;
use fastctx_types::session::{Happened, Watched, What};

use crate::clients::{Conversations, Dice, Opening, Seen, type_and_send};
use crate::setup::{Lab, PATROL, SOAK, answers, machine, reopen};

/// One thing made to go wrong, and when.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Fault {
    pub kind: &'static str,
    #[serde(skip)]
    pub began: Instant,
    #[serde(skip)]
    pub ended: Option<Instant>,
    pub from: jiff::Timestamp,
    pub to: Option<jiff::Timestamp>,
}

/// Everything made to go wrong, in order.
#[derive(Default)]
pub(crate) struct Faults(Mutex<Vec<Fault>>);

impl Faults {
    /// `kind` begins now; the returned index ends it.
    pub fn begin(&self, kind: &'static str) -> usize {
        say(&format!("fault: {kind} begins"));
        let mut faults = self.0.lock().expect("never poisoned");
        faults.push(Fault { kind, began: Instant::now(), ended: None, from: jiff::Timestamp::now(), to: None });
        faults.len() - 1
    }

    pub fn end(&self, index: usize) {
        let mut faults = self.0.lock().expect("never poisoned");
        let fault = &mut faults[index];
        fault.ended = Some(Instant::now());
        fault.to = Some(jiff::Timestamp::now());
        say(&format!("fault: {} ends", fault.kind));
    }

    /// Something that happens at once.
    pub fn once(&self, kind: &'static str) {
        let index = self.begin(kind);
        self.end(index);
    }

    pub fn all(&self) -> Vec<Fault> {
        self.0.lock().expect("never poisoned").clone()
    }
}

pub(crate) fn say(line: &str) {
    println!("{} {line}", jiff::Timestamp::now());
}

/// The node's line cut, once.
pub(crate) fn cut(lab: &Lab, faults: &Faults) {
    faults.once("cut");
    lab.node_line.cut();
}

/// The node's line cut four times within a minute: with the first time it
/// came this hour, five connections, one more than a steady machine makes
/// before its owner is told.
pub(crate) async fn drops(lab: &Lab, faults: &Faults) {
    let index = faults.begin("drops");
    for _ in 0..4 {
        lab.node_line.cut();
        // Back on its line before the next cut, so each is a connection.
        let began = Instant::now();
        tokio::time::sleep(Duration::from_secs(2)).await;
        while began.elapsed() < Duration::from_secs(30) && !answers(lab.soak.root(), Duration::from_secs(1), true).await
        {
        }
        tokio::time::sleep(Duration::from_secs(8)).await;
    }
    faults.end(index);
}

/// The node's line frozen both ways for `held`: nothing gets through, and
/// nothing closes, until the hub hears nothing for its whole silence.
pub(crate) async fn frozen(lab: &Lab, faults: &Faults, held: Duration) {
    let index = faults.begin("frozen");
    lab.node_line.freeze(true);
    tokio::time::sleep(held).await;
    lab.node_line.freeze(false);
    faults.end(index);
}

/// Every phone's line cut at once: all of them come back together.
pub(crate) fn phones_cut(lab: &Lab, faults: &Faults) {
    faults.once("phones cut");
    lab.device_line.cut();
}

/// The node's disk at `iops` operations a second for `held`, then back to
/// a small server's.
pub(crate) async fn disk(
    lab: &Lab,
    faults: &Faults,
    kind: &'static str,
    iops: u64,
    held: Duration,
) -> anyhow::Result<()> {
    lab.soak.disk(iops)?;
    let index = faults.begin(kind);
    tokio::time::sleep(held).await;
    let back = lab.soak.disk(lab.config.iops);
    faults.end(index);
    back
}

/// A job that runs away on the node's own machine: two loops that only
/// burn its cores and a flood of output, for `held`.
pub(crate) async fn runaway(
    local: &World,
    seen: &Seen,
    resident: &ResidentId,
    faults: &Faults,
    held: Duration,
) -> anyhow::Result<()> {
    let seconds = held.as_secs();
    let script = format!(
        "soak runaway [[later: on {SOAK} <<'RUNAWAY'\n\
         timeout {seconds} sh -c 'while :; do :; done' &\n\
         timeout {seconds} sh -c 'while :; do :; done' &\n\
         timeout {seconds} yes soak-runaway-output | head -c 400000000\n\
         wait\n\
         RUNAWAY]]"
    );
    // Made only once it is sent: a node too busy to take it is told by
    // the check that every fault was made, and by what was said.
    if !type_and_send(local, seen, &["type", "send"], resident, None, &script).await {
        let failed = seen.failed();
        let said = failed.get("send").or_else(|| failed.get("type")).map(|(_, said)| said.clone());
        anyhow::bail!("the runaway job was not sent: {}", said.unwrap_or_else(|| "no outcome came".into()));
    }
    let index = faults.begin("runaway");
    tokio::time::sleep(held).await;
    faults.end(index);
    Ok(())
}

/// A thousand calls at once: from this machine's interface and from phones.
pub(crate) async fn storm(lab: &Lab, conversations: &Conversations, seen: Arc<Seen>, faults: &Faults) {
    let index = faults.begin("storm");
    let mut calls = tokio::task::JoinSet::new();
    for n in 0..600u64 {
        let (local, seen) = (lab.local.clone(), seen.clone());
        calls.spawn(async move {
            match n % 3 {
                0 => drop(seen.time("storm", local.world_now()).await),
                1 => drop(seen.time("storm", local.who(None, true)).await),
                _ => drop(seen.time("storm", local.nodes()).await),
            }
        });
    }
    let mut dice = Dice::new(crate::clients::seed() ^ 0x5707);
    let mut devices = Vec::new();
    for (directory, _) in lab.phones.iter().filter(|(_, owner)| owner.as_str() == "alice").take(4) {
        let Ok((device, world)) = reopen(directory) else { continue };
        for _ in 0..100 {
            let Some(resident) = conversations.any(&mut dice) else { break };
            let (world, seen) = (world.clone(), seen.clone());
            calls.spawn(async move {
                drop(seen.time("storm.phone", world.offers(resident, Some(machine(SOAK).into()))).await);
            });
        }
        devices.push(device);
    }
    // Each call ends with its outcome, at most `OUTCOME` on.
    while calls.join_next().await.is_some() {}
    for device in devices {
        device.disconnect();
    }
    faults.end(index);
}

/// The patrol: a job on the patrol machine stepping back onto the node
/// every second, its steps' facts going back to the conversation that
/// started it, until `stop` is there on the patrol machine.
pub(crate) struct Patrol {
    pub resident: ResidentId,
    stop: PathBuf,
    /// Where the patrol counts its steps as it takes them, on its machine.
    count: PathBuf,
    pub began: Instant,
}

/// The steps a patrol took, by its own count, and those its conversation
/// was told of.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) struct Steps {
    pub taken: u64,
    pub told: u64,
}

impl Patrol {
    pub async fn start(lab: &Lab, seen: &Seen, open: &Opening, conversations: &Conversations) -> anyhow::Result<Self> {
        let stop = lab.patrol_root.join("patrol.stop");
        let count = lab.patrol_root.join("patrol.count");
        let _ = std::fs::remove_file(&stop);
        let _ = std::fs::remove_file(&count);
        let resident = open
            .open_soon(&lab.local, seen, 0)
            .await
            .map_err(|said| anyhow::anyhow!("opening the patrol's conversation: {said}"))?;
        conversations.rest(&resident, Duration::from_secs(120)).await;
        let script = format!(
            "soak patrol [[later: on {PATROL} <<'PATROL'\n\
             i=0\n\
             while test ! -e '{stop}'; do\n\
             i=$((i+1))\n\
             on {SOAK} <<STEP\n\
             echo patrol-$i\n\
             STEP\n\
             echo $i > '{count}'\n\
             sleep 1\n\
             done\n\
             PATROL]]",
            stop = stop.display(),
            count = count.display()
        );
        anyhow::ensure!(
            type_and_send(&lab.local, seen, &["type", "send"], &resident, None, &script).await,
            "the patrol was not sent"
        );
        Ok(Self { resident, stop, count, began: Instant::now() })
    }

    /// Ends the patrol: the steps it took by its own count, and those its
    /// conversation was told of once it ended.
    pub async fn finish(&self, local: &World) -> anyhow::Result<Steps> {
        std::fs::write(&self.stop, b"")?;
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(180) {
            if let Some(facts) = last_run(local, &self.resident).await
                && !matches!(facts.state, fastctx_types::run::RunState::Running)
            {
                let folded: u64 = facts.folded_steps.iter().map(|summary| summary.count).sum();
                let taken = std::fs::read_to_string(&self.count)?.trim().parse()?;
                return Ok(Steps { taken, told: facts.steps.len() as u64 + folded });
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        anyhow::bail!("the patrol's run did not end within three minutes of being told to")
    }
}

/// The facts of the last run the conversation finished, as its history
/// shows them.
async fn last_run(local: &World, resident: &ResidentId) -> Option<RunFacts> {
    let mut watch = local.watch(resident.clone(), None).await.ok()?;
    let events = tokio::time::timeout(Duration::from_secs(40), async {
        while let Ok(Some(event)) = watch.next().await {
            if let Watched::History { events } = event {
                return Some(events);
            }
        }
        None
    })
    .await
    .ok()??;
    events.into_iter().rev().find_map(|event| match event.happened {
        Happened::Finished { item, .. } => match item.what {
            What::Tool(tool) => tool.run.map(|run| *run),
            _ => None,
        },
        _ => None,
    })
}

/// The node ended at once and started again: how long until it takes
/// requests again.
pub(crate) async fn restart(lab: &Lab, faults: &Faults) -> anyhow::Result<Duration> {
    let index = faults.begin("restart");
    lab.soak.kill();
    let began = Instant::now();
    lab.soak.start()?;
    let answered = answers(lab.soak.root(), Duration::from_secs(60), false).await;
    let took = began.elapsed();
    anyhow::ensure!(answered, "the node did not take requests within a minute of starting again");
    answers(lab.soak.root(), Duration::from_secs(120), true).await;
    faults.end(index);
    Ok(took)
}

/// The node's own log, from `root`.
pub(crate) fn node_log(root: &Path) -> String {
    std::fs::read_to_string(root.join("world/node.log")).unwrap_or_default()
}
