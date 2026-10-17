//! Letters sent through the hub in their thousands, and whether each came
//! once.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_net::link::Link;
use fastctx_net::message::LetterId;
use fastctx_types::access::Purpose;
use fastctx_types::remote::Letter;
use fastctx_types::{AccountName, MachineName, ResidentId};
use tokio::task::JoinSet;

use crate::lab::Lab;
use crate::world::World;

/// Letters on their way from their senders.
pub struct Sending {
    senders: JoinSet<anyhow::Result<()>>,
    /// The ids of every letter sent, those that kept coming among them
    /// once `sent` returned.
    pub wanted: Vec<String>,
    /// For letters that keep coming (`flow`).
    flow: Option<Arc<Flow>>,
}

/// Letters that keep coming past the first ones, until they are stopped.
struct Flow {
    going: AtomicBool,
    /// How many were sent, from every sender.
    sent: AtomicUsize,
    /// The ids of those sent past the first ones.
    more: Mutex<Vec<String>>,
    receiver: Arc<Lab>,
}

impl Sending {
    /// Stops letters that keep coming, and waits until every letter is on
    /// its sender's disk.
    pub async fn sent(&mut self) -> anyhow::Result<()> {
        if let Some(flow) = &self.flow {
            flow.going.store(false, Ordering::SeqCst);
        }
        while let Some(sender) = self.senders.join_next().await {
            sender??;
        }
        if let Some(flow) = self.flow.take() {
            self.wanted.extend(flow.more.lock().expect("never poisoned").drain(..));
        }
        Ok(())
    }

    /// Whether some letter sent so far has not come to `lab` yet.
    pub fn on_their_way(&self, lab: &Lab) -> bool {
        let more = self.flow.as_ref().map_or(0, |flow| flow.more.lock().expect("never poisoned").len());
        lab.received() < self.wanted.len() + more
    }
}

/// At most this many past the first ones are on their way at once, so the
/// letters that keep coming pile up nowhere, however slowly they go.
const FLOWING: usize = 600;
/// At most this many are sent past the first ones, from each sender: more
/// than a fast machine takes while a scenario's trouble lasts.
const MOST_MORE: usize = 200_000;

/// Sends `count` letters from each of `from` to `to`, named after `tag`,
/// pausing now and then as a machine at work does.
pub fn send(world: &World, from: &[&str], to: &str, tag: &str, count: usize) -> Sending {
    let to = world.machine(to).name.clone();
    send_to(world, from, &to, tag, count)
}

/// As `send`, to a member that need not be a machine of this process.
pub fn send_to(world: &World, from: &[&str], to: &MachineName, tag: &str, count: usize) -> Sending {
    let mut wanted = Vec::new();
    let mut senders = JoinSet::new();
    for (index, name) in from.iter().enumerate() {
        let ids: Vec<String> = (0..count).map(|i| format!("{tag}-{name}-{i}")).collect();
        wanted.extend(ids.iter().cloned());
        let link = world.machine(name).link();
        let (to, account) = (to.clone(), world.account.clone());
        senders.spawn(sender(link, to, account, ids, index as u64 + 1));
    }
    Sending { senders, wanted, flow: None }
}

/// As `send`, but the letters keep coming past the first `count` from each
/// until `Sending::sent`: whatever happens meanwhile happens while letters
/// are on their way, however fast the machines take them.
pub fn flow(world: &World, from: &[&str], to: &str, tag: &str, count: usize) -> Sending {
    let receiver = world.machine(to);
    let flow = Arc::new(Flow {
        going: AtomicBool::new(true),
        sent: AtomicUsize::new(0),
        more: Mutex::default(),
        receiver: receiver.lab.clone(),
    });
    let mut wanted = Vec::new();
    let mut senders = JoinSet::new();
    for (index, name) in from.iter().enumerate() {
        let prefix = format!("{tag}-{name}");
        wanted.extend((0..count).map(|i| format!("{prefix}-{i}")));
        let link = world.machine(name).link();
        let (to, account) = (receiver.name.clone(), world.account.clone());
        senders.spawn(flowing(link, to, account, prefix, count, index as u64 + 1, flow.clone()));
    }
    Sending { senders, wanted, flow: Some(flow) }
}

async fn sender(
    link: Arc<Link>,
    to: MachineName,
    account: AccountName,
    ids: Vec<String>,
    seed: u64,
) -> anyhow::Result<()> {
    let resident = ResidentId::try_from("codex-7f3a".to_string()).expect("a resident's id");
    let mut pause = crate::Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
    for (i, id) in ids.into_iter().enumerate() {
        let id = LetterId::try_from(id).expect("a letter's id");
        let letter = Letter::Left { resident: resident.clone() };
        // A resident's leaving is told as an echo of the sender's own.
        link.send_letter(&to, &account, Purpose::Echo, id, letter)?;
        if i % 100 == 99 {
            tokio::time::sleep(Duration::from_millis(pause.between(5, 60))).await;
        }
    }
    Ok(())
}

async fn flowing(
    link: Arc<Link>,
    to: MachineName,
    account: AccountName,
    prefix: String,
    count: usize,
    seed: u64,
    flow: Arc<Flow>,
) -> anyhow::Result<()> {
    let resident = ResidentId::try_from("codex-7f3a".to_string()).expect("a resident's id");
    let mut pause = crate::Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
    for i in 0..count + MOST_MORE {
        let more = i >= count;
        if more {
            while flow.going.load(Ordering::SeqCst)
                && flow.sent.load(Ordering::SeqCst) > flow.receiver.received() + FLOWING
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if !flow.going.load(Ordering::SeqCst) {
                break;
            }
        }
        let id = format!("{prefix}-{i}");
        let letter_id = LetterId::try_from(id.clone()).expect("a letter's id");
        // Counted before it goes, as it can come before its sender says so.
        flow.sent.fetch_add(1, Ordering::SeqCst);
        if more {
            flow.more.lock().expect("never poisoned").push(id);
        }
        // A resident's leaving is told as an echo of the sender's own.
        link.send_letter(&to, &account, Purpose::Echo, letter_id, Letter::Left { resident: resident.clone() })?;
        if more {
            // Never resting, so a receiver faster than its senders still
            // has letters on their way at every moment.
            tokio::task::yield_now().await;
        } else if i % 100 == 99 {
            tokio::time::sleep(Duration::from_millis(pause.between(5, 60))).await;
        }
    }
    Ok(())
}

/// What came of the letters wanted at one machine.
pub struct Arrived {
    pub missing: Vec<String>,
    /// How many came more than once.
    pub twice: usize,
    pub took: Duration,
}

/// Waits for every letter of `wanted` to come to `lab`, for `within` at most.
pub async fn arrived(lab: &Lab, wanted: &[String], within: Duration) -> Arrived {
    let began = Instant::now();
    loop {
        let got = lab.got();
        let missing: Vec<String> = wanted.iter().filter(|id| !got.contains_key(*id)).cloned().collect();
        if missing.is_empty() || began.elapsed() > within {
            let twice = wanted.iter().filter(|id| got.get(*id).is_some_and(|count| *count > 1)).count();
            return Arrived { missing, twice, took: began.elapsed() };
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

impl Arrived {
    /// The first few missing, for the report.
    pub fn first_missing(&self) -> String {
        let shown: Vec<&str> = self.missing.iter().take(8).map(String::as_str).collect();
        format!(
            "{} missing{}",
            self.missing.len(),
            if shown.is_empty() { String::new() } else { format!(", first {}", shown.join(" ")) }
        )
    }
}
