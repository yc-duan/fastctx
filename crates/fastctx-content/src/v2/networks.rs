//! The networks a device's library holds scopes of (design §4.5; the user,
//! 2026-11-12: what a network had on a machine goes when the machine leaves
//! it). Each scope a device begins or installs is written as one hub's
//! network's, and the library keeps whether the machine is on each network
//! and the term it is on it under. Leaving ends the term and revokes every
//! scope of the network held here in one transaction, this device's writes
//! not yet uploaded kept aside first; joining begins a new term, whatever an
//! earlier term left still held revoked first. A scope is begun or
//! installed only under the term current in its own transaction, so a
//! producer or driver that looked before the machine left begins nothing
//! after it.

use super::Library;
use super::scopes::{install_in, open_holding};
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// A network this machine is on, under the term it was on it when asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    hub: String,
    term: i64,
}

/// A held scope as the hub last listed it to this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// Its descriptor as the hub gave it.
    pub description: Vec<u8>,
    /// The prefixes of its stream classes this device may hold, one a line.
    pub allowed: Option<String>,
    /// Those logged in here who read it, one `account@opened` a line.
    pub readers: Option<String>,
}

impl Network {
    /// The hub whose network it is.
    pub fn hub(&self) -> &str {
        &self.hub
    }
}

/// The term of `hub`'s network here and whether the machine is on it.
fn standing(db: &Connection, hub: &str) -> fastctx_storage::Result<Option<(i64, bool)>> {
    Ok(db
        .prepare_cached("SELECT term, joined FROM networks WHERE hub = ?1")?
        .query_row([hub], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? == 1)))
        .optional()?)
}

/// Whether the machine is on `network` under its term still.
fn under(db: &Connection, network: &Network) -> fastctx_storage::Result<()> {
    if standing(db, &network.hub)? == Some((network.term, true)) {
        Ok(())
    } else {
        Err(invalid(format!("this machine has left the network of {} since", network.hub)))
    }
}

/// `hub`'s network written with `term`, joined or not.
fn stand(db: &Connection, hub: &str, term: i64, joined: bool) -> fastctx_storage::Result<()> {
    db.prepare_cached(
        "INSERT INTO networks (hub, term, joined) VALUES (?1, ?2, ?3) ON CONFLICT (hub) DO UPDATE SET term = ?2, joined = ?3",
    )?
    .execute(params![hub, term, joined as i64])?;
    Ok(())
}

/// Every scope of `hub`'s network held here revoked, `device`'s writes not
/// yet uploaded kept first, but for the streams `stays` names by scope and
/// stream (`Library::revoke_keeping_but`): how many were held.
fn revoke_all(db: &Connection, hub: &str, device: &[u8; 32], stays: &dyn Fn(&str, &str) -> bool) -> fastctx_storage::Result<usize> {
    let keys: Vec<String> = {
        let mut statement = db.prepare_cached("SELECT key FROM scopes WHERE hub = ?1 AND holding IS NOT NULL ORDER BY key")?;
        let rows = statement.query_map([hub], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for key in &keys {
        super::letgo::revoke_keeping_in(db, key, device, &|stream| stays(key, stream))?;
    }
    Ok(keys.len())
}

impl Library {
    /// The network of `hub` under its current term, if this machine is on it.
    pub fn network(&self, hub: &str) -> Result<Option<Network>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(match standing(db, &hub)? {
                Some((term, true)) => Some(Network { hub, term }),
                _ => None,
            })
        })?)
    }

    /// This machine is on `hub`'s network: under the term it is on it
    /// already, or a new one, whatever an earlier term left held here
    /// revoked first, `device`'s writes not yet uploaded kept.
    /// `anew`, a membership begun now, as a login admits the machine again,
    /// always starts a new term: whatever the one before left — a leaving
    /// not done yet, as when the hub removed the machine — ends with it
    /// (AJ04).
    pub fn join(&self, hub: &str, device: &[u8; 32], anew: bool) -> Result<Network> {
        self.join_but(hub, device, anew, |_, _| false)
    }

    /// `join`, with nothing kept of the streams `stays` names by scope and
    /// stream where an earlier term's holdings are revoked
    /// (`revoke_keeping_but`).
    pub fn join_but(
        &self,
        hub: &str,
        device: &[u8; 32],
        anew: bool,
        stays: impl Fn(&str, &str) -> bool + Send + 'static,
    ) -> Result<Network> {
        let (hub, device) = (hub.to_string(), *device);
        let joined = self.database().write_now_at(Priority::Control, 1 << 20, move |db| {
            let before = standing(db, &hub)?;
            if let Some((term, true)) = before
                && !anew
            {
                return Ok(Network { hub, term });
            }
            db.prepare_cached("DELETE FROM feeds WHERE hub = ?1")?.execute([&hub])?;
            revoke_all(db, &hub, &device, &stays)?;
            let term = before.map_or(1, |(term, _)| term + 1);
            stand(db, &hub, term, true)?;
            Ok(Network { hub, term })
        })?;
        self.wrote();
        Ok(joined)
    }

    /// This machine left `hub`'s network: its term ends and every scope of
    /// it held here is revoked in the same transaction, `device`'s writes
    /// not yet uploaded kept first. How many scopes were held.
    pub fn leave(&self, hub: &str, device: &[u8; 32]) -> Result<usize> {
        self.leave_but(hub, device, |_, _| false)
    }

    /// `leave`, with nothing kept of the streams `stays` names by scope and
    /// stream (`revoke_keeping_but`).
    pub fn leave_but(&self, hub: &str, device: &[u8; 32], stays: impl Fn(&str, &str) -> bool + Send + 'static) -> Result<usize> {
        let (hub, device) = (hub.to_string(), *device);
        let held = self.database().write_now_at(Priority::Control, 1 << 20, move |db| {
            let term = standing(db, &hub)?.map_or(1, |(term, _)| term + 1);
            stand(db, &hub, term, false)?;
            // Where it stood in the network's libraries goes with it: joined
            // again, it starts from nothing.
            db.prepare_cached("DELETE FROM feeds WHERE hub = ?1")?.execute([&hub])?;
            revoke_all(db, &hub, &device, &stays)
        })?;
        self.wrote();
        Ok(held)
    }

    /// The scopes of `hub`'s network held here under a hub's descriptor
    /// that were not pulled whole in their holding period yet (§28.1): each
    /// is asked after by itself, not by the change numbers alone, until it
    /// is.
    pub fn unpulled_of(&self, hub: &str) -> Result<std::collections::BTreeSet<String>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key FROM scopes WHERE hub = ?1 AND state = 'live' AND holding IS NOT NULL AND pulled IS NOT holding",
            )?;
            let rows = statement.query_map([&hub], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// No scope of `hub`'s network counts as pulled whole here any more: each
    /// is pulled by itself again before its changes carry it (a tier made
    /// heavier holds what the lighter one never took, design §28.42).
    pub fn forget_pulled(&self, hub: &str) -> Result<()> {
        let hub = hub.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            db.prepare_cached("UPDATE scopes SET pulled = NULL WHERE hub = ?1")?.execute([&hub])?;
            Ok(())
        })?)
    }

    /// The scope `scope` counts as pulled whole here no more: it is pulled by
    /// itself again, each stream from where it stands here, before its
    /// changes carry it — what passed it by while this machine took nothing
    /// of it in comes down then, though nothing of it moves again.
    pub fn forget_pulled_of(&self, scope: &str) -> Result<()> {
        let scope = scope.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            db.prepare_cached("UPDATE scopes SET pulled = NULL WHERE key = ?1")?.execute([&scope])?;
            Ok(())
        })?)
    }

    /// Whether every stream of `scope` was pulled whole in its holding period
    /// here: what orders each is known.
    pub fn is_pulled(&self, scope: &str) -> Result<bool> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT 1 FROM scopes WHERE key = ?1 AND holding IS NOT NULL AND pulled IS holding")?
                .query_row([&scope], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// Every stream of `scope` was pulled whole in its holding period
    /// `holding`; a later period is pulled whole again.
    pub fn pulled(&self, scope: &str, holding: i64) -> Result<()> {
        let scope = scope.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            db.prepare_cached("UPDATE scopes SET pulled = ?2 WHERE key = ?1 AND holding = ?2")?.execute(params![scope, holding])?;
            Ok(())
        })?)
    }

    /// The description `scope` is held here under, what the hub last said
    /// this device may hold of it, and who logged in here it said reads it,
    /// if it is held.
    pub fn held_listing(&self, scope: &str) -> Result<Option<Listing>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT description, allowed, readers FROM scopes WHERE key = ?1 AND holding IS NOT NULL")?
                .query_row([&scope], |row| {
                    Ok(Listing {
                        description: row.get::<_, Option<Vec<u8>>>(0)?.unwrap_or_default(),
                        allowed: row.get(1)?,
                        readers: row.get(2)?,
                    })
                })
                .optional()?)
        })?)
    }

    /// What the hub says this device may hold of the held `scope` now, and
    /// who logged in here reads it: once what it may hold is otherwise than
    /// it was, the scope is pulled whole again (HH04); its readers changing
    /// pull nothing.
    pub fn allow(&self, scope: &str, allowed: &str, readers: &str) -> Result<()> {
        let (scope, allowed, readers) = (scope.to_string(), allowed.to_string(), readers.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 256 + allowed.len() + readers.len(), move |db| {
            db.prepare_cached(
                "UPDATE scopes SET allowed = ?2, readers = ?3, pulled = CASE WHEN allowed IS ?2 THEN pulled ELSE NULL END
                 WHERE key = ?1 AND holding IS NOT NULL",
            )?
            .execute(params![scope, allowed, readers])?;
            Ok(())
        })?)
    }

    /// The scopes of `hub`'s network held here under a hub's descriptor,
    /// each with its descriptor and holding period.
    pub fn live_scopes_of(&self, hub: &str) -> Result<Vec<(String, Vec<u8>, i64)>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key, description, holding FROM scopes WHERE hub = ?1 AND state = 'live' AND holding IS NOT NULL ORDER BY key",
            )?;
            let rows = statement.query_map([&hub], |row| Ok((row.get(0)?, row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(), row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The hub whose network `scope` belongs to here, if it is held.
    pub fn hub_of(&self, scope: &str) -> Result<Option<String>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT hub FROM scopes WHERE key = ?1 AND holding IS NOT NULL AND hub != ''")?
                .query_row([&scope], |row| row.get(0))
                .optional()?)
        })?)
    }

    /// The hubs of the networks this machine is on, as the library has them.
    pub fn joined(&self) -> Result<Vec<String>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT hub FROM networks WHERE joined = 1 ORDER BY hub")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The hubs of networks this machine is not on with a scope still held
    /// here: what a leaving stopped midway left.
    pub fn left_held(&self) -> Result<Vec<String>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT DISTINCT s.hub FROM scopes s WHERE s.holding IS NOT NULL AND s.hub != ''
                 AND NOT EXISTS (SELECT 1 FROM networks n WHERE n.hub = s.hub AND n.joined = 1) ORDER BY s.hub",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// A scope this device begins on `network` before its hub described it,
    /// with what it proposes the hub describe it as (§2.1, R26): readable
    /// here, never uploaded or supplied until a descriptor installs it. One
    /// held keeps its period; one never described whose period a leaving
    /// ended begins a new period; one a hub described, not held now, is held
    /// again only when a hub's listing installs it.
    pub fn propose_on(&self, network: &Network, key: &str, proposal: Vec<u8>) -> Result<i64> {
        let (network, key) = (network.clone(), key.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 256 + proposal.len(), move |db| {
            under(db, &network)?;
            let here: Option<(i64, Option<i64>, bool)> = db
                .prepare_cached("SELECT id, holding, described FROM scopes WHERE key = ?1")?
                .query_row([&key], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? == 1)))
                .optional()?;
            match here {
                Some((id, Some(_), _)) => {
                    db.prepare_cached("UPDATE scopes SET description = ?2, hub = ?3 WHERE id = ?1 AND state = 'provisional'")?
                        .execute(params![id, proposal, network.hub])?;
                    Ok(id)
                }
                Some((id, None, true)) => Ok(id),
                Some((id, None, false)) => {
                    let holding = open_holding(db, id, 0)?;
                    db.prepare_cached("UPDATE scopes SET state = 'provisional', description = ?2, hub = ?3, holding = ?4 WHERE id = ?1")?
                        .execute(params![id, proposal, network.hub, holding])?;
                    Ok(id)
                }
                None => {
                    db.prepare_cached("INSERT INTO scopes (key, description, state, holding, hub) VALUES (?1, ?2, 'provisional', NULL, ?3)")?
                        .execute(params![key, proposal, network.hub])?;
                    let id = db.last_insert_rowid();
                    let holding = open_holding(db, id, 0)?;
                    db.prepare_cached("UPDATE scopes SET holding = ?2 WHERE id = ?1")?.execute(params![id, holding])?;
                    Ok(id)
                }
            }
        })?)
    }

    /// Installs a scope this device proposed as `install_on` does, only
    /// while it is held as proposed still, or installed since: one let go
    /// meanwhile — its conversation deleted — is not held anew (`None`).
    pub fn install_proposed(&self, network: &Network, key: &str, description: Vec<u8>, acl_revision: i64) -> Result<Option<i64>> {
        let (network, key) = (network.clone(), key.to_string());
        let holding = self.database().write_now_at(Priority::Normal, 256 + description.len(), move |db| {
            under(db, &network)?;
            match super::scopes::held(db, &key)? {
                Some((_, state, Some(_))) if state == "provisional" || state == "live" => {
                    install_in(db, &key, &network.hub, description, acl_revision).map(Some)
                }
                _ => Ok(None),
            }
        })?;
        if holding.is_some() {
            self.wrote();
        }
        Ok(holding)
    }

    /// Installs a scope of `network` a hub listed, as `install_on` does,
    /// but not one this device proposed and no hub described yet — judged
    /// in the same transaction: that one is described by this device as it
    /// began it (`None`).
    pub fn install_listed(&self, network: &Network, key: &str, description: Vec<u8>, acl_revision: i64) -> Result<Option<i64>> {
        let (network, key) = (network.clone(), key.to_string());
        let holding = self.database().write_now_at(Priority::Normal, 256 + description.len(), move |db| {
            under(db, &network)?;
            if let Some((_, state, Some(_))) = super::scopes::held(db, &key)?
                && state == "provisional"
            {
                return Ok(None);
            }
            install_in(db, &key, &network.hub, description, acl_revision).map(Some)
        })?;
        if holding.is_some() {
            self.wrote();
        }
        Ok(holding)
    }

    /// Installs a scope of `network` under its hub-signed description as
    /// `install` does, only while the machine is on the network under the
    /// term it was asked under.
    pub fn install_on(&self, network: &Network, key: &str, description: Vec<u8>, acl_revision: i64) -> Result<i64> {
        let (network, key) = (network.clone(), key.to_string());
        let holding = self.database().write_now_at(Priority::Normal, 256 + description.len(), move |db| {
            under(db, &network)?;
            install_in(db, &key, &network.hub, description, acl_revision)
        })?;
        self.wrote();
        Ok(holding)
    }
}
