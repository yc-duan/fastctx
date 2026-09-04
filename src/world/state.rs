//! `~/.fastctx/world/state.json`: the member's mutable link state. Counters are reserved in
//! blocks so a crash can never reuse a sequence number or an envelope counter.

use super::{NetworkMode, WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const STATE_FILE_VERSION: u32 = 1;
/// Counter values reserved per write of the state file.
const COUNTER_RESERVATION: u64 = 1_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct NodeState {
    /// Format version of this file.
    #[serde(default)]
    pub(crate) v: u32,
    /// Highest reliable sequence number this member has assigned towards the hub.
    #[serde(default)]
    pub(crate) send_seq: u64,
    /// Highest hub sequence number this member has processed.
    #[serde(default)]
    pub(crate) recv_seq: u64,
    /// Reserved ceiling for the envelope counter `n`; the live value starts above the last
    /// persisted ceiling.
    #[serde(default)]
    pub(crate) n_reserved: u64,
    /// Reserved ceiling for request ids.
    #[serde(default)]
    pub(crate) request_reserved: u64,
    /// Highest envelope counter accepted from each sender.
    #[serde(default)]
    pub(crate) seen: BTreeMap<String, u64>,
    /// Network mode that last connected, so `auto` starts there.
    #[serde(default)]
    pub(crate) last_network: Option<NetworkMode>,
    #[serde(default)]
    pub(crate) members_version: u64,
    #[serde(default)]
    pub(crate) grant_version: u64,
    /// Version of the last inventory this member published.
    #[serde(default)]
    pub(crate) inventory_version: u64,
    /// Times this connection was replaced by another process using the same key; resets
    /// after a stable hour.
    #[serde(default)]
    pub(crate) replaced_count: u32,
}

impl NodeState {
    pub(crate) fn load(paths: &WorldPaths) -> Result<Self, String> {
        let Some(bytes) = read_optional(&paths.state)? else {
            return Ok(Self {
                v: STATE_FILE_VERSION,
                ..Self::default()
            });
        };
        let state: Self = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.state)
            )
        })?;
        if state.v > STATE_FILE_VERSION {
            return Err(format!(
                "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
                crate::paths::display_path(&paths.state),
                state.v,
                STATE_FILE_VERSION
            ));
        }
        Ok(state)
    }

    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        let mut copy = self.clone();
        copy.v = STATE_FILE_VERSION;
        let json = serde_json::to_vec_pretty(&copy)
            .map_err(|error| format!("Cannot encode the World state: {error}"))?;
        write_atomic(&paths.state, &json)
    }
}

/// Persistent counters handed out in memory and reserved on disk in blocks.
#[derive(Debug)]
pub(crate) struct Counters {
    next_n: u64,
    n_ceiling: u64,
    next_request: u64,
    request_ceiling: u64,
}

impl Counters {
    /// Starts above the last persisted ceilings and reserves the next blocks.
    pub(crate) fn resume(state: &mut NodeState) -> Self {
        let next_n = state.n_reserved + 1;
        let next_request = state.request_reserved + 1;
        state.n_reserved += COUNTER_RESERVATION;
        state.request_reserved += COUNTER_RESERVATION;
        Self {
            next_n,
            n_ceiling: state.n_reserved,
            next_request,
            request_ceiling: state.request_reserved,
        }
    }

    /// The next envelope counter; `true` means the state file must be saved first.
    pub(crate) fn next_n(&mut self, state: &mut NodeState) -> (u64, bool) {
        let value = self.next_n;
        self.next_n += 1;
        let mut reserve = false;
        if self.next_n >= self.n_ceiling {
            self.n_ceiling += COUNTER_RESERVATION;
            state.n_reserved = self.n_ceiling;
            reserve = true;
        }
        (value, reserve)
    }

    pub(crate) fn next_request(&mut self, state: &mut NodeState) -> (u64, bool) {
        let value = self.next_request;
        self.next_request += 1;
        let mut reserve = false;
        if self.next_request >= self.request_ceiling {
            self.request_ceiling += COUNTER_RESERVATION;
            state.request_reserved = self.request_ceiling;
            reserve = true;
        }
        (value, reserve)
    }
}

#[cfg(test)]
mod tests {
    use super::{Counters, NodeState};

    #[test]
    fn counters_never_repeat_across_a_restart() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::world::WorldPaths::from_control(
            &crate::control::paths::ControlPaths::for_home(temp.path()),
        );
        paths.ensure().unwrap();
        let mut state = NodeState::load(&paths).unwrap();
        let mut counters = Counters::resume(&mut state);
        state.save(&paths).unwrap();
        let (first, _) = counters.next_n(&mut state);
        let (second, _) = counters.next_n(&mut state);
        assert_eq!((first, second), (1, 2));

        let mut reloaded = NodeState::load(&paths).unwrap();
        let mut counters = Counters::resume(&mut reloaded);
        let (after_restart, _) = counters.next_n(&mut reloaded);
        assert!(after_restart > second);
    }
}
