//! Each completed wake is a recoverable snapshot, published before the current pointer.
use fastctx_agent::{
    model::Usage,
    state::{load, save},
};
use fastctx_types::sentinel::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub definition: Definition,
    pub sequence: u64,
    pub wake_id: String,
    pub pending: Option<WakeIntent>,
    pub plan: String,
    pub notes: String,
    pub reflexes: Vec<Reflex>,
    pub recent: Vec<Observation>,
    pub conclusion: Conclusion,
    pub usage: Usage,
    pub requests: u32,
    pub unreported_requests: u32,
    pub reserved_tokens: u64,
    pub elapsed_ms: u64,
    pub obstacle: Option<String>,
    pub obstacle_count: u32,
    pub finished: bool,
    #[serde(default)]
    pub completed_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WakeIntent {
    pub id: String,
    pub reason: WakeReason,
    pub at: i64,
}

impl State {
    pub fn new(definition: Definition) -> Self {
        Self {
            definition,
            sequence: 0,
            wake_id: String::new(),
            pending: None,
            plan: String::new(),
            notes: String::new(),
            reflexes: vec![],
            recent: vec![],
            conclusion: Conclusion::Watching,
            usage: Usage::default(),
            requests: 0,
            unreported_requests: 0,
            reserved_tokens: 0,
            elapsed_ms: 0,
            obstacle: None,
            obstacle_count: 0,
            finished: false,
            completed_at: 0,
        }
    }
}
pub struct Store {
    directory: PathBuf,
}
impl Store {
    /// The caller supplies a private directory and serializes this sentinel's writes.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn load(&self) -> Result<Option<State>, String> {
        let mut current: Option<State> = load(&self.directory.join("state.json"))?;
        let entries = match std::fs::read_dir(self.directory.join("history")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|v| v != "json") {
                continue;
            }
            let sequence = path
                .file_stem()
                .and_then(|v| v.to_str())
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or("invalid sentinel history filename")?;
            if current.as_ref().is_none_or(|s| s.sequence < sequence || s.sequence == sequence && s.pending.is_some()) {
                let recovered: State = load(&path)?.ok_or("sentinel history disappeared")?;
                if recovered.sequence != sequence || recovered.pending.is_some() {
                    return Err("invalid completed sentinel snapshot".into());
                }
                if current.as_ref().is_some_and(|s| {
                    s.sequence == sequence && s.pending.as_ref().is_some_and(|p| p.id != recovered.wake_id)
                }) {
                    continue;
                }
                current = Some(recovered);
            }
        }
        Ok(current)
    }
    pub fn checkpoint(&self, state: &State) -> Result<(), String> {
        save(&self.directory.join("state.json"), state)
    }
    pub fn commit(&self, state: &State) -> Result<(), String> {
        save(&self.directory.join("history").join(format!("{:020}.json", state.sequence)), state)?;
        save(&self.directory.join("state.json"), state)
    }
    pub fn run_path(&self, sequence: u64) -> PathBuf {
        self.directory.join("runs").join(format!("{sequence:020}.json"))
    }
    /// Counts a wake that began at `began` and took `usage` over
    /// `elapsed_ms` into its day, so the totals of a span need no wake read
    /// (`world-senses.md`, 全部观测); days older than `TALLY_DAYS` go.
    pub fn tallied(&self, began: i64, usage: &Usage, elapsed_ms: u64) -> Result<(), String> {
        let path = self.directory.join("tally.json");
        let mut days: Vec<fastctx_types::sentinel_control::WakeDay> = load(&path)?.unwrap_or_default();
        let day = began.div_euclid(DAY_MS) * DAY_MS;
        days.retain(|kept| kept.day > day - TALLY_DAYS * DAY_MS);
        let at = match days.iter().position(|kept| kept.day == day) {
            Some(at) => at,
            None => {
                days.push(fastctx_types::sentinel_control::WakeDay { day, ..Default::default() });
                days.sort_by_key(|kept| kept.day);
                days.iter().position(|kept| kept.day == day).expect("just kept")
            }
        };
        let kept = &mut days[at];
        kept.wakes += 1;
        kept.input = kept.input.saturating_add(usage.input);
        kept.output = kept.output.saturating_add(usage.output);
        kept.elapsed_ms = kept.elapsed_ms.saturating_add(elapsed_ms);
        save(&path, &days)
    }
    /// The days counted since `since` (milliseconds since the epoch), the
    /// one it falls in among them.
    pub fn tally(&self, since: i64) -> Result<Vec<fastctx_types::sentinel_control::WakeDay>, String> {
        let mut days: Vec<fastctx_types::sentinel_control::WakeDay> =
            load(&self.directory.join("tally.json"))?.unwrap_or_default();
        days.retain(|kept| kept.day + DAY_MS > since);
        Ok(days)
    }
}

const DAY_MS: i64 = 86_400_000;

/// How many days of wakes a sentinel keeps counted: more than the overview
/// looks back over.
const TALLY_DAYS: i64 = 62;
