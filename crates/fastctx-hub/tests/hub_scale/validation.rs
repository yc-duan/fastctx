//! Independent assertions for the deliberately simple synthetic network.
//! No production authorization or delivery decision is used as its oracle.

use crate::{client::Stats, fixture::Identity};
use fastctx_net::directory::View;
use fastctx_net::envelope::Envelope;
use fastctx_types::MachineName;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

pub struct Checks {
    index: usize,
    members: usize,
    stable_seen: HashSet<MachineName>,
    senders: HashMap<usize, u64>,
}

fn number(name: &MachineName) -> anyhow::Result<usize> {
    name.as_str()
        .strip_prefix('m')
        .ok_or_else(|| anyhow::anyhow!("unknown synthetic member {name}"))?
        .parse()
        .map_err(Into::into)
}
fn account(index: usize, members: usize) -> usize {
    if index >= members { (index - members) / 2 } else { index / 2 }
}

impl Checks {
    pub fn new(index: usize, members: usize) -> Self {
        Self { index, members, stable_seen: HashSet::new(), senders: HashMap::new() }
    }

    pub fn view(&mut self, identity: &Identity, view: &View, stats: &Stats) -> anyhow::Result<()> {
        anyhow::ensure!(view.me == identity.name, "wrong member's view");
        let me = view.directory.member(&identity.name).ok_or_else(|| anyhow::anyhow!("view omitted its reader"))?;
        anyhow::ensure!(me.online, "connected reader {} was declared offline", identity.name);
        for member in &view.directory.members {
            if member.name.as_str() == "controller" {
                continue;
            }
            let other = number(&member.name)?;
            anyhow::ensure!(
                account(other, self.members) == account(self.index, self.members),
                "{} received another account's member {}",
                identity.name,
                member.name
            );
            self.presence(&member.name, member.online)?;
            if self.index < 2
                && member.name.as_str() == "m0"
                && stats.fault.as_deref() != Some("view")
                && let Some(version) = member
                    .group
                    .as_deref()
                    .and_then(|group| group.strip_prefix("group-"))
                    .and_then(|number| number.parse::<u64>().ok())
            {
                stats.observed_groups[self.index].fetch_max(version, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    pub fn presence(&mut self, machine: &MachineName, online: bool) -> anyhow::Result<()> {
        if machine.as_str() == "controller" {
            return Ok(());
        }
        if number(machine)? < self.members {
            return Ok(());
        }
        if online {
            self.stable_seen.insert(machine.clone());
        } else {
            anyhow::ensure!(
                !self.stable_seen.contains(machine),
                "continuously connected member {machine} was announced offline"
            );
        }
        Ok(())
    }

    pub fn letter(&mut self, identity: &Identity, message: &Envelope) -> anyhow::Result<bool> {
        let body = message.body::<fastctx_net::message::LetterBody>().map_err(anyhow::Error::msg)?;
        let parts: Vec<_> = body.id.as_str().split('-').collect();
        anyhow::ensure!(parts.len() == 3 && parts[0] == "scale", "unrequested letter {}", body.id.as_str());
        let sender: usize = parts[1].parse()?;
        let sequence: u64 = parts[2].parse()?;
        let recipient = if sender >= self.members { sender - self.members } else { sender ^ 1 };
        anyhow::ensure!(
            recipient == self.index
                && message.to == identity.name
                && message.from.as_str() == format!("m{sender}")
                && message.account.as_ref() == Some(&identity.account)
                && message.n == sequence,
            "letter {} reached the wrong member or changed identity",
            body.id.as_str()
        );
        anyhow::ensure!(
            matches!(body.letter, fastctx_types::remote::Letter::Left { ref resident } if resident.as_str() == "codex-7f3a"),
            "letter body changed"
        );
        let last = self.senders.entry(sender).or_default();
        if sequence <= *last {
            return Ok(false);
        }
        anyhow::ensure!(
            sequence == *last + 1,
            "sender m{sender} delivered {sequence} after {last}: lost or reordered letter"
        );
        *last = sequence;
        Ok(true)
    }
}

/// The fixture keeps each ordinary member's visible neighborhood fixed. A
/// tenfold increase must cost less than half the proportional growth, leaving
/// room for cache effects without accepting an O(network) read path. This
/// compares thread CPU time; wall latency is recorded separately because CPU
/// quota throttling can suspend an otherwise constant-time operation.
pub fn compare(before: &serde_json::Value, after: &serde_json::Value) -> anyhow::Result<()> {
    let small = before["members_reconnected"].as_u64().ok_or_else(|| anyhow::anyhow!("baseline lacks member count"))?;
    let large = after["members_reconnected"].as_u64().ok_or_else(|| anyhow::anyhow!("result lacks member count"))?;
    anyhow::ensure!(small > 0 && large >= small * 10, "cost comparison needs at least tenfold growth");
    anyhow::ensure!(before["cgroup"] == after["cgroup"], "cost comparison used different resource limits");
    for key in ["cpu_max", "memory_max", "swap_max"] {
        anyhow::ensure!(
            before["enforced_limits"][key] == after["enforced_limits"][key],
            "cost comparison changed {key}"
        );
    }
    for metric in ["view", "presence"] {
        let a = &before["after_storm"][metric]["cpu"];
        let b = &after["after_storm"][metric]["cpu"];
        anyhow::ensure!(
            a["count"].as_u64().unwrap_or(0) >= 1000 && b["count"].as_u64().unwrap_or(0) >= 1000,
            "insufficient {metric} CPU samples (comparison requires Linux)"
        );
        for statistic in ["mean_us", "p99_us"] {
            let a = a[statistic].as_u64().ok_or_else(|| anyhow::anyhow!("baseline lacks {metric} {statistic}"))?.max(1);
            let b = b[statistic].as_u64().ok_or_else(|| anyhow::anyhow!("result lacks {metric} {statistic}"))?.max(1);
            anyhow::ensure!(
                (b as u128) * (small as u128) * 2 < (a as u128) * (large as u128),
                "{metric} {statistic} grew from {a} to {b} with {small} to {large} members; not demonstrably sublinear"
            );
            println!("PASS {metric} CPU {statistic}: {a} -> {b} us for {small} -> {large} members");
        }
    }
    Ok(())
}
