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
/// Earlier records failing to move this long are told of.
const MIGRATION: Duration = Duration::from_secs(10 * 60);

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
    /// How many earlier files it had set aside as unreadable when last heard.
    set_aside: u32,
    /// Whether how its migration goes was heard on this connection yet.
    migration_heard: bool,
    /// Since when some of its earlier records fail to move, and whether that was told.
    migration: Option<(Instant, bool)>,
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
        // Earlier files it could not read are records lost to it too: told
        // as more are set aside, while their originals are kept to look at.
        let set_aside = health.migrating.as_ref().map_or(0, |migrating| migrating.damaged);
        if set_aside > self.set_aside {
            hub.troubles.raise(
                hub,
                machine,
                TroubleKind::Damaged,
                Detail { count: Some(set_aside), ..Detail::default() },
            );
        }
        self.set_aside = set_aside;
        // A release that tells how its migration goes tells it whole, set
        // aside or not: kept, and what it said before closed once it is over.
        if let Some(migrating) = &health.migrating {
            let first = !self.migration_heard;
            self.migration_heard = true;
            hub.troubles.migrating(hub, name, key, migrating, first);
        }
        match health.migrating.as_ref().map(|migrating| migrating.failed).filter(|failed| *failed > 0) {
            Some(failed) => {
                let (since, told) = self.migration.get_or_insert((now, false));
                if !*told && now.duration_since(*since) >= MIGRATION {
                    *told = true;
                    let count = Some(failed);
                    hub.troubles.raise(hub, machine, TroubleKind::Migration, Detail { count, ..Detail::default() });
                }
            }
            None => self.migration = None,
        }
        // The rules a machine broke are a developer's reading, kept in its
        // own diagnostics (`fastctx doctor readings`), never told as trouble
        // (`specs/world-foundation.md`, 规矩由机器守).
    }
}
