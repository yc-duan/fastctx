//! Letters sent through the hub in their thousands, and whether each came
//! once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_net::link::Link;
use fastctx_net::message::LetterId;
use fastctx_types::access::Purpose;
use fastctx_types::remote::Letter;
use fastctx_types::{AccountName, MachineName, ResidentId};
use tokio::task::JoinHandle;

use crate::lab::Lab;
use crate::world::World;

/// Letters on their way from their senders.
pub struct Sending {
    senders: Vec<JoinHandle<()>>,
    /// The ids of every letter sent.
    pub wanted: Vec<String>,
}

impl Sending {
    /// Waits until every letter is on its sender's disk.
    pub async fn sent(&mut self) {
        for sender in self.senders.drain(..) {
            let _ = sender.await;
        }
    }
}

/// Sends `count` letters from each of `from` to `to`, named after `tag`,
/// pausing now and then as a machine at work does.
pub fn send(world: &World, from: &[&str], to: &str, tag: &str, count: usize) -> Sending {
    let to = world.machine(to).name.clone();
    send_to(world, from, &to, tag, count)
}

/// As `send`, to a member that need not be a machine of this process.
pub fn send_to(world: &World, from: &[&str], to: &MachineName, tag: &str, count: usize) -> Sending {
    let mut wanted = Vec::new();
    let mut senders = Vec::new();
    for (index, name) in from.iter().enumerate() {
        let ids: Vec<String> = (0..count).map(|i| format!("{tag}-{name}-{i}")).collect();
        wanted.extend(ids.iter().cloned());
        let link = world.machine(name).link();
        let (to, account) = (to.clone(), world.account.clone());
        senders.push(tokio::spawn(sender(link, to, account, ids, index as u64 + 1)));
    }
    Sending { senders, wanted }
}

async fn sender(link: Arc<Link>, to: MachineName, account: AccountName, ids: Vec<String>, seed: u64) {
    let resident = ResidentId::try_from("codex-7f3a".to_string()).expect("a resident's id");
    let mut pause = crate::Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
    for (i, id) in ids.into_iter().enumerate() {
        let id = LetterId::try_from(id).expect("a letter's id");
        let letter = Letter::Left { resident: resident.clone() };
        // A resident's leaving is told as an echo of the sender's own.
        link.send_letter(&to, &account, Purpose::Echo, id, letter).expect("a letter is kept");
        if i % 100 == 99 {
            tokio::time::sleep(Duration::from_millis(pause.between(5, 60))).await;
        }
    }
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
