//! How each machine's own work goes, as its heartbeats tell
//! (`specs/world-engine.md`, 量与告警): busy or stuck beside online, told to
//! those who see the machine only when it changes, and what goes on too long
//! told to those who hold it (`crate::troubles`). Each session keeps what its
//! heartbeats told, so a heartbeat takes no shared lock and the cost follows
//! the changes, not the heartbeats.

use std::time::{Duration, Instant};

use fastctx_net::sign::PublicKey;
use fastctx_types::MachineName;
use fastctx_types::health::{Health, Strain};
use fastctx_types::trouble::TroubleKind;

use crate::Hub;
use crate::troubles::Detail;

/// Work stuck this long is told of.
const STUCK: Duration = Duration::from_secs(60);
/// Rules broken in each of this many windows of `WINDOW` running are told of.
const BROKEN_WINDOWS: u32 = 3;
const WINDOW: Duration = Duration::from_secs(5 * 60);

/// What one session's heartbeats told of its machine's health.
#[derive(Default)]
pub(crate) struct Heard {
    strain: Option<Strain>,
    /// Since when some of its work is stuck, and whether that was told.
    stuck: Option<(Instant, bool)>,
    /// The bound near its limit last told, while one is.
    near: Option<String>,
    /// Whether a damaged store of its records was told.
    damaged: bool,
    /// Since when this window runs, the rules broken by its start, and in how
    /// many windows running more were broken; from the first health told.
    window: Option<(Instant, u64, u32)>,
}

impl Heard {
    /// `name`'s session `generation` told `health` with a heartbeat; none
    /// from a release that does not tell it.
    pub(crate) fn heartbeat(
        &mut self,
        hub: &Hub,
        name: &MachineName,
        key: PublicKey,
        generation: u64,
        health: Option<&Health>,
    ) {
        let strain = health.and_then(Health::strain);
        if strain != self.strain {
            self.strain = strain;
            let changed = {
                let mut sessions = hub.sessions();
                match sessions.get_mut(name).filter(|session| session.generation == generation) {
                    Some(session) => {
                        session.strain = strain;
                        true
                    }
                    None => false,
                }
            };
            if changed {
                hub.present(name);
            }
        }
        let Some(health) = health else { return };
        let now = Instant::now();
        let machine = Some((name, key));
        if health.stuck > 0 {
            let (since, told) = self.stuck.get_or_insert((now, false));
            if !*told && now.duration_since(*since) >= STUCK {
                *told = true;
                let seconds = Some(now.duration_since(*since).as_secs());
                hub.troubles.raise(hub, machine, TroubleKind::Stuck, Detail { seconds, ..Detail::default() });
            }
        } else {
            self.stuck = None;
        }
        match &health.near {
            Some(near) if self.near.as_deref() != Some(near.bound.as_str()) => {
                self.near = Some(near.bound.clone());
                hub.troubles.raise(
                    hub,
                    machine,
                    TroubleKind::Near,
                    Detail { near: Some(near.clone()), ..Detail::default() },
                );
            }
            Some(_) => {}
            None => self.near = None,
        }
        if health.damaged.is_some() && !self.damaged {
            self.damaged = true;
            hub.troubles.raise(hub, machine, TroubleKind::Damaged, Detail::default());
        }
        let (began, broken, running) = self.window.get_or_insert((now, health.violations, 0));
        if now.duration_since(*began) >= WINDOW {
            *running = if health.violations > *broken { *running + 1 } else { 0 };
            *began = now;
            *broken = health.violations;
            if *running >= BROKEN_WINDOWS {
                *running = 0;
                hub.troubles.raise(hub, machine, TroubleKind::Violations, Detail::default());
            }
        }
    }
}
