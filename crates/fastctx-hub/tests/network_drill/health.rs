//! Whether a scenario left the hub as it found it, by the hub's own account
//! (`health.json`): no call waiting, no transfer paired or waiting, no
//! letter held, being written or on its way, its records written, its
//! queues empty, and every machine's one session open.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::world::Hub;

/// Waits for the hub to say, in a health written after `since`, that
/// nothing is left over and `sessions` sessions are open; or says what was
/// left in the last one it wrote.
pub async fn settled(hub: &Hub, since: jiff::Timestamp, sessions: u64, within: Duration) -> Result<Duration, String> {
    let began = Instant::now();
    let mut last = "no health written".to_string();
    loop {
        if let Some(health) = hub.health()
            && written_after(&health, since)
        {
            let left = left_over(&health, sessions);
            if left.is_empty() {
                return Ok(began.elapsed());
            }
            last = left.join(", ");
        }
        if began.elapsed() > within {
            return Err(last);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// What `health` says is left over, each as `name count`.
fn left_over(health: &Value, sessions: u64) -> Vec<String> {
    let mut left = Vec::new();
    if health["sessions"].as_u64() != Some(sessions) {
        left.push(format!("sessions {} where {sessions} are connected", health["sessions"]));
    }
    for (group, name) in [
        ("calls", "pending"),
        ("transfers", "waiting"),
        ("transfers", "live"),
        ("transfers", "registered"),
        ("letters", "held"),
        ("letters", "being_written"),
        ("letters", "on_their_way"),
        ("letters", "records_behind"),
        ("queues", "urgent_frames"),
        ("queues", "letters"),
        ("queues", "notices"),
    ] {
        let count = &health[group][name];
        if count.as_u64() != Some(0) {
            left.push(format!("{group}.{name} {count}"));
        }
    }
    left
}

/// The first health the hub writes after `since`, within `within`.
pub async fn fresh(hub: &Hub, since: jiff::Timestamp, within: Duration) -> Option<Value> {
    let began = Instant::now();
    loop {
        if let Some(health) = hub.health()
            && written_after(&health, since)
        {
            return Some(health);
        }
        if began.elapsed() > within {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether the hub wrote `health` after `since`: one written before says
/// nothing of what happened since, however well it reads.
pub fn written_after(health: &Value, since: jiff::Timestamp) -> bool {
    health["at"].as_str().and_then(|at| at.parse::<jiff::Timestamp>().ok()).is_some_and(|at| at > since)
}

/// How many letters the hub holds now for every member together.
pub fn held(hub: &Hub) -> Option<u64> {
    hub.health()?["letters"]["held"].as_u64()
}
