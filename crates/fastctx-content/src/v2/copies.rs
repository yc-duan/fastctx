//! Scopes found by the end of their key (plan 3-5, design §28.30): a
//! conversation of another machine is looked up here by its resident's id,
//! the machine it was born on or the body it is in not known to the reader.

use super::Library;
use super::frame::Header;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// A scope held here, as a reader decides whether to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldScope {
    pub key: String,
    pub holding: i64,
    /// The holding period its first pull was done in, when it was.
    pub pulled: Option<i64>,
    /// The classes of its streams the hub lets this device hold, one
    /// prefix a line.
    pub allowed: Option<String>,
    /// Those logged in here the hub last said read it, one `account@opened`
    /// a line.
    pub readers: Option<String>,
    /// The hub that described it, and its description as the hub gave it.
    pub hub: Option<String>,
    pub description: Option<Vec<u8>>,
}

/// What copies held here take, as near as is cheaply known: the bytes of
/// their frames in their present holdings, and of the objects here they
/// refer to, each counted once however many of them refer to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopySize {
    pub frames: u64,
    pub objects: u64,
}

/// What came of letting a copy go (`Library::drop_copy`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// Its holding ended: its frames, and the objects nothing else holds, go
    /// with the purge.
    Gone,
    /// Kept, and why.
    Kept(String),
}

/// Why the copy of `scope` held here in `holding` is to be kept, if it is:
/// it is not held so, a stream of it is ordered by `device`, a frame of it
/// was signed by `device`, a write of `device`'s waits in it to be sent again
/// or is kept for its user, or it keeps aside what a stream of it held before
/// it went another way (`evidence`) — perhaps the one copy of that anywhere.
fn kept_why(db: &Connection, scope: &str, holding: i64, device: &[u8; 32]) -> fastctx_storage::Result<Option<String>> {
    let Some((id, _, Some(current))) = super::scopes::held(db, scope)? else {
        return Ok(Some("it is not held here".into()));
    };
    if current != holding {
        return Ok(Some("it was held anew since it was looked at".into()));
    }
    if !super::device::own(db, id, holding, device)?.is_empty() {
        return Ok(Some("this machine orders a stream of it".into()));
    }
    let waiting: i64 = db
        .prepare_cached("SELECT COUNT(*) FROM resubmits r JOIN streams s ON s.id = r.stream WHERE s.scope = ?1")?
        .query_row([id], |row| row.get(0))?;
    if waiting > 0 {
        return Ok(Some("writes of this machine wait in it to be sent again".into()));
    }
    let ops: i64 = db.prepare_cached("SELECT COUNT(*) FROM op_records WHERE scope = ?1")?.query_row([id], |row| row.get(0))?;
    if ops > 0 {
        return Ok(Some("it holds writes this machine made".into()));
    }
    let aside: i64 = db
        .prepare_cached("SELECT COUNT(*) FROM evidence e JOIN streams s ON s.id = e.stream WHERE s.scope = ?1 AND e.holding = ?2")?
        .query_row(params![id, holding], |row| row.get(0))?;
    if aside > 0 {
        return Ok(Some("it keeps aside what a stream of it held before it went another way".into()));
    }
    let mut frames = db
        .prepare_cached("SELECT f.header FROM frames f JOIN streams s ON s.id = f.stream WHERE s.scope = ?1 AND f.holding = ?2")?;
    let mut rows = frames.query(params![id, holding])?;
    while let Some(row) = rows.next()? {
        let header: Vec<u8> = row.get(0)?;
        match Header::decode(&header) {
            Ok(header) if header.device != *device => {}
            Ok(_) => return Ok(Some("this machine wrote to it".into())),
            Err(_) => return Ok(Some("a frame of it cannot be read".into())),
        }
    }
    Ok(None)
}

impl Library {
    /// What the copy of `scope` held here takes; nothing where it is not held.
    pub fn copy_size(&self, scope: &str) -> Result<CopySize> {
        self.copies_size(&[scope.to_string()])
    }

    /// What the copies `scopes` held here take together; nothing of one not
    /// held.
    pub fn copies_size(&self, scopes: &[String]) -> Result<CopySize> {
        let scopes = scopes.to_vec();
        Ok(self.database().read_now(move |db| {
            let (mut frames, mut objects) = (0i64, 0i64);
            let mut counted: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
            for scope in &scopes {
                let Some((id, _, Some(holding))) = super::scopes::held(db, scope)? else { continue };
                frames += db
                    .prepare_cached(
                        "SELECT COALESCE(SUM(length(f.header) + length(f.body)), 0) FROM frames f
                         JOIN streams s ON s.id = f.stream WHERE s.scope = ?1 AND f.holding = ?2",
                    )?
                    .query_row(params![id, holding], |row| row.get::<_, i64>(0))?;
                // Each object the copy's frames refer to, found by its hash
                // (`+o.state`): found by their state, every object present
                // here was gone through for each copy counted.
                let mut statement = db.prepare_cached(
                    "SELECT o.hash, o.size FROM objects o WHERE +o.state = 1 AND o.hash IN
                     (SELECT DISTINCT fo.hash FROM frame_objects fo JOIN streams s ON s.id = fo.stream
                      WHERE s.scope = ?1 AND fo.holding = ?2)",
                )?;
                let rows = statement.query_map(params![id, holding], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)))?;
                for row in rows {
                    let (hash, size) = row?;
                    if counted.insert(hash) {
                        objects += size;
                    }
                }
            }
            Ok(CopySize { frames: frames.max(0) as u64, objects: objects.max(0) as u64 })
        })?)
    }

    /// The room letting go the copies `scopes` held here would give back:
    /// their frames in their present holdings, and the objects they refer to
    /// that nothing else here refers to, holds or leases, each counted once.
    pub fn copies_freed(&self, scopes: &[String]) -> Result<CopySize> {
        let scopes = scopes.to_vec();
        Ok(self.database().read_now(move |db| {
            let mut frames = 0i64;
            let mut ids = std::collections::BTreeSet::new();
            let mut hashes: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
            for scope in &scopes {
                let Some((id, _, Some(holding))) = super::scopes::held(db, scope)? else { continue };
                ids.insert(id);
                frames += db
                    .prepare_cached(
                        "SELECT COALESCE(SUM(length(f.header) + length(f.body)), 0) FROM frames f
                         JOIN streams s ON s.id = f.stream WHERE s.scope = ?1 AND f.holding = ?2",
                    )?
                    .query_row(params![id, holding], |row| row.get::<_, i64>(0))?;
                let mut statement = db.prepare_cached(
                    "SELECT DISTINCT fo.hash FROM frame_objects fo JOIN streams s ON s.id = fo.stream WHERE s.scope = ?1 AND fo.holding = ?2",
                )?;
                let rows = statement.query_map(params![id, holding], |row| row.get::<_, Vec<u8>>(0))?;
                for row in rows {
                    hashes.insert(row?);
                }
            }
            let mut objects = 0i64;
            for hash in hashes {
                let size: Option<i64> = db
                    .prepare_cached("SELECT size FROM objects WHERE hash = ?1 AND state = 1")?
                    .query_row([&hash], |row| row.get(0))
                    .optional()?;
                let Some(size) = size else { continue };
                let mut elsewhere = false;
                for query in [
                    "SELECT DISTINCT s.scope FROM frame_objects f JOIN streams s ON s.id = f.stream WHERE f.hash = ?1",
                    "SELECT DISTINCT scope FROM object_refs WHERE hash = ?1 AND count > 0",
                ] {
                    let mut statement = db.prepare_cached(query)?;
                    let rows = statement.query_map([&hash], |row| row.get::<_, i64>(0))?;
                    for row in rows {
                        if !ids.contains(&row?) {
                            elsewhere = true;
                        }
                    }
                }
                let held: bool = db
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM object_holds WHERE hash = ?1)
                             OR EXISTS (SELECT 1 FROM leases WHERE hash = ?1 AND expires >= ?2)",
                    )?
                    .query_row(params![&hash, super::objects::now()], |row| row.get(0))?;
                if !elsewhere && !held {
                    objects += size;
                }
            }
            Ok(CopySize { frames: frames.max(0) as u64, objects: objects.max(0) as u64 })
        })?)
    }

    /// Why the copy of `scope` held here in `holding` would be kept by
    /// `drop_copy` now, if it would.
    pub fn copy_kept(&self, scope: &str, holding: i64, device: &[u8; 32]) -> Result<Option<String>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| kept_why(db, &scope, holding, &device))?)
    }

    /// Lets go the copy of `scope` held here in `holding` where nothing of
    /// it is this device's, `device`, and nothing of it is kept aside here
    /// (`kept_why`). Checked and let go in one transaction, its holding ended
    /// as a revocation ends it; its purge follows (`purge`).
    pub fn drop_copy(&self, scope: &str, holding: i64, device: &[u8; 32]) -> Result<Dropped> {
        let (scope, device) = (scope.to_string(), *device);
        let dropped = self.database().write_now_at(Priority::Normal, 1 << 16, move |db| {
            if let Some(why) = kept_why(db, &scope, holding, &device)? {
                return Ok(Dropped::Kept(why));
            }
            super::scopes::revoke_in(db, &scope)?;
            Ok(Dropped::Gone)
        })?;
        if dropped == Dropped::Gone {
            self.wrote();
        }
        Ok(dropped)
    }

    /// Whether this library knows a scope keyed `key`, in any state: one of
    /// `keys_ending`, asked by its key alone.
    pub fn has_key(&self, key: &str) -> Result<bool> {
        let key = key.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db.prepare_cached("SELECT EXISTS (SELECT 1 FROM scopes WHERE key = ?1)")?.query_row(params![key], |row| row.get(0))?)
        })?)
    }

    /// The keys of every scope this library knows, in any state, ending
    /// with `tail`.
    pub fn keys_ending(&self, tail: &str) -> Result<Vec<String>> {
        let tail = tail.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key FROM scopes
                 WHERE length(key) > length(?1) AND substr(key, length(key) - length(?1) + 1) = ?1
                 ORDER BY key",
            )?;
            let rows = statement.query_map(params![tail], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Every live scope held here.
    pub fn held_scopes(&self) -> Result<Vec<HeldScope>> {
        self.scopes_ending("")
    }

    /// The live scopes held here whose key ends with `tail`.
    pub fn scopes_ending(&self, tail: &str) -> Result<Vec<HeldScope>> {
        let tail = tail.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT key, holding, pulled, allowed, hub, description, readers FROM scopes
                 WHERE state = 'live' AND holding IS NOT NULL AND length(key) > length(?1)
                   AND (?1 = '' OR substr(key, length(key) - length(?1) + 1) = ?1)
                 ORDER BY key",
            )?;
            let rows = statement.query_map(params![tail], |row| {
                Ok(HeldScope {
                    key: row.get(0)?,
                    holding: row.get(1)?,
                    pulled: row.get(2)?,
                    allowed: row.get(3)?,
                    hub: row.get(4)?,
                    description: row.get(5)?,
                    readers: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }
}
