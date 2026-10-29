//! Home state derived from authoritative assignments and completed wake snapshots.
use super::*;
use fastctx_agent::state::load;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Alarm { index: usize },
    Satisfied { reason: String, evidence: Vec<Observation> },
    Unable { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub registration: Registration,
    pub at: i64,
    pub due: i64,
    pub event: Event,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Offline {
    pub since: i64,
    pub check_at: i64,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Ledger {
    pub alarms: BTreeSet<usize>,
    pub offline: BTreeMap<String, Offline>,
    pub notifications: Vec<Notification>,
    pub delivered: BTreeSet<String>,
}
impl Ledger {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(load(&store.directory().join("home.json"))?.unwrap_or_default())
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        save(&store.directory().join("home.json"), self)
    }
    pub fn alarms(&mut self, registration: &Registration, state: &State, until: i64) {
        if matches!(state.conclusion, Conclusion::Withdrawn) {
            return;
        }
        for (index, due) in state.definition.at.iter().enumerate() {
            if *due <= until && self.alarms.insert(index) {
                self.add(registration, state, format!("alarm-{index}"), *due, *due, Event::Alarm { index });
            }
        }
    }
    fn add(&mut self, registration: &Registration, state: &State, key: String, at: i64, due: i64, event: Event) {
        // The assignment file is supplied before any wake can produce a notification.
        let id =
            format!("{}-{}-{}-{key}", scope_key(&registration.scope), state.definition.id, state.definition.generation);
        if !self.notifications.iter().any(|n| n.id == id) {
            self.notifications.push(Notification { id, registration: registration.clone(), at, due, event });
        }
    }
    pub fn conclusion(&mut self, registration: &Registration, state: &State) {
        let event = match &state.conclusion {
            Conclusion::Satisfied { reason, evidence } => {
                Event::Satisfied { reason: reason.clone(), evidence: evidence.clone() }
            }
            Conclusion::Unable { reason } => Event::Unable { reason: reason.clone() },
            _ => return,
        };
        self.add(
            registration,
            state,
            format!("wake-{}", state.sequence),
            state.completed_at,
            state.completed_at,
            event,
        );
    }
}

fn scope_key(scope: &HubAccount) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(scope).expect("scope serializes")))
}
impl Host {
    pub(super) fn assignment(&self, scope: &HubAccount, id: &SentinelId) -> PathBuf {
        self.scope_dir(scope).join(id.as_str()).join("assignment.json")
    }
    /// Where the account's sentinels living here keep what they do.
    pub(super) fn scope_dir(&self, scope: &HubAccount) -> PathBuf {
        self.root.join(scope_key(scope))
    }
    pub(super) fn store(&self, registration: &Registration) -> Store {
        Store::new(
            self.assignment(&registration.scope, &registration.definition.id)
                .parent()
                .expect("assignment directory")
                .join(registration.definition.generation.to_string()),
        )
    }
    /// Local assignments are discovery hints only. Every effect rereads the account record.
    pub fn known(&self) -> Result<Vec<Registration>, String> {
        fn directories(path: &Path) -> Result<Vec<PathBuf>, String> {
            match std::fs::read_dir(path) {
                Ok(entries) => entries
                    .map(|e| match e {
                        Ok(e) => e.file_type().map(|t| (e.path(), t)).map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    })
                    .filter_map(|e| match e {
                        Ok((p, t)) if t.is_dir() => Some(Ok(p)),
                        Ok(_) => None,
                        Err(e) => Some(Err(e)),
                    })
                    .collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(e.to_string()),
            }
        }
        let mut result = Vec::new();
        for scope in directories(&self.root)? {
            for id in directories(&scope)? {
                if let Some(registration) = load(&id.join("assignment.json"))? {
                    result.push(registration);
                }
            }
        }
        Ok(result)
    }
    pub(super) async fn withdraw_local(&self, scope: &HubAccount, id: &SentinelId) -> Result<(), String> {
        let Some(registration): Option<Registration> = load(&self.assignment(scope, id))? else {
            return Ok(());
        };
        if registration.scope != *scope || registration.definition.id != *id {
            return Err("local sentinel identity mismatch".into());
        }
        let store = self.store(&registration);
        if let Some(mut state) = store.load()? {
            // A sentinel that finished on its own keeps the conclusion it reached.
            if !state.finished {
                state.finished = true;
                state.completed_at = self.runtime.now();
                state.conclusion = Conclusion::Withdrawn;
                state.reflexes.clear();
                state.pending = None;
                state.sequence += 1;
                store.commit(&state)?;
            }
            self.synchronize(&registration, &state, &Ledger::load(&store)?).await?;
        }
        // Nothing acts for it here any more; its generations keep what it saw.
        match std::fs::remove_file(self.assignment(scope, id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}
pub(super) fn summary(state: &State) -> Summary {
    let (phase, reason) = match &state.conclusion {
        Conclusion::Satisfied { reason, .. } => (Phase::Satisfied, Some(reason.clone())),
        Conclusion::Unable { reason } => (Phase::Unable, Some(reason.clone())),
        Conclusion::Withdrawn => (Phase::Withdrawn, None),
        Conclusion::NotJudged { reason } => (Phase::Watching, Some(reason.clone())),
        _ if state.finished => (Phase::Satisfied, None),
        _ => (Phase::Watching, None),
    };
    Summary {
        definition: state.definition.clone(),
        phase,
        reason,
        plan: state.plan.clone(),
        latest: state.recent.last().cloned(),
    }
}
