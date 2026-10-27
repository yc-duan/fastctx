//! Positive evidence required before retiring a supervisor-managed worker.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub struct IdleWorker {
    pub id: String,
    pub pid: u32,
}

#[derive(Deserialize)]
struct Job {
    #[serde(rename = "inFlight")]
    in_flight: Option<InFlight>,
    tempo: Option<String>,
    needs: Option<String>,
}

/// What Claude Code writes in a job's `needs` while it waits for its first
/// prompt (2.1.303 and 2.1.310). Such a job is written without `inFlight`:
/// nothing runs in it until a prompt comes.
const WAITING_FOR_FIRST_PROMPT: &str = "send a prompt to start";

#[derive(Deserialize)]
struct InFlight {
    tasks: u64,
    kinds: Vec<String>,
}

/// Unreadable or unfamiliar evidence is an error, never permission to retire.
/// An absent pins file means no pins, as in the supervisor. `queued` is
/// ignored because Claude can leave a stale queue snapshot after settling.
pub fn idle_worker(home: &Path, session: &str, listed: &str) -> Result<Option<IdleWorker>, String> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(listed).map_err(|error| error.to_string())?;
    let mut matching =
        rows.iter().filter(|row| row["kind"] == "background" && (row["sessionId"] == session || row["id"] == session));
    let Some(row) = matching.next() else { return Ok(None) };
    if matching.next().is_some() {
        return Err("Claude Code lists multiple supervisor jobs for the conversation".into());
    }
    let id = row["id"]
        .as_str()
        .filter(|id| id.len() == 8 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("Claude Code returned no valid supervisor job ID")?;
    if row["status"] != "idle" {
        return Ok(None);
    }
    let Some(pid) = row["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid != 0) else {
        return Ok(None);
    };
    let state = home.join("jobs").join(id).join("state.json");
    let job: Job = read(&state)?;
    let busy = match &job.in_flight {
        Some(in_flight) => in_flight.tasks != 0 || in_flight.kinds.iter().any(|kind| kind == "session_cron"),
        None if job.tempo.as_deref() == Some("blocked") && job.needs.as_deref() == Some(WAITING_FOR_FIRST_PROMPT) => {
            false
        }
        None => return Err(format!("{} says nothing of the work in flight", state.display())),
    };
    let pins_path = home.join("jobs/pins.json");
    let pins: Vec<String> = match std::fs::read(&pins_path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("reading {}: {error}", pins_path.display()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("reading {}: {error}", pins_path.display())),
    };
    if busy || pins.iter().any(|pin| pin == id) {
        return Ok(None);
    }
    Ok(Some(IdleWorker { id: id.into(), pid }))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("reading {}: {error}", path.display()))
}
