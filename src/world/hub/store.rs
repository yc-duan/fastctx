//! Hub storage: one redb file holding metadata the hub routes on and ciphertext it keeps
//! for members. Every write is one transaction; redb serializes writers, so the hub never
//! has to reason about interleaved updates.

use crate::world::envelope::Envelope;
use crate::world::keys::SealedKey;
use crate::world::messages::{Event, SignedGrant, SignedRecord};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const MEMBERS: TableDefinition<&str, &[u8]> = TableDefinition::new("members");
const MEMBER_KEYS: TableDefinition<&str, &str> = TableDefinition::new("member_keys");
const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");
const OUTBOX: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("outbox");
const INVITES: TableDefinition<&str, &[u8]> = TableDefinition::new("invites");
const KEYS: TableDefinition<(u32, &str), &[u8]> = TableDefinition::new("keys");
const GRANTS: TableDefinition<&str, &[u8]> = TableDefinition::new("grants");
const INVENTORY: TableDefinition<&str, &[u8]> = TableDefinition::new("inventory");
const EVENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("events");

/// Format version of the whole database; readers refuse a newer one.
const STORE_FORMAT: u64 = 1;
/// Events kept before the oldest are dropped.
pub(crate) const EVENT_RETENTION: u64 = 100_000;
/// Reliable messages queued per member before new ones are refused.
pub(crate) const OUTBOX_LIMIT: u64 = 10_000;

pub(crate) mod meta {
    pub(crate) const FORMAT: &str = "format";
    pub(crate) const WORLD_ID: &str = "world_id";
    pub(crate) const BOOTSTRAP_ADMISSION: &str = "bootstrap_admission";
    pub(crate) const BOOTSTRAP_USED: &str = "bootstrap_used";
    pub(crate) const MEMBERS_VERSION: &str = "members_version";
    pub(crate) const GRANT_VERSION: &str = "grant_version";
    pub(crate) const EVENT_SEQ: &str = "event_seq";
    pub(crate) const KEY_EPOCH: &str = "key_epoch";
    pub(crate) const ROTATION_PENDING: &str = "rotation_pending";
    pub(crate) const HUB_N: &str = "hub_n";
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberRow {
    pub(crate) name: String,
    /// Ed25519 public key, base64.
    pub(crate) node_pub: String,
    /// X25519 wrap public key, base64.
    pub(crate) wrap_pub: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    pub(crate) admitted_at: String,
    /// The record the member published about itself; absent until its first `member_publish`.
    #[serde(default)]
    pub(crate) signed: Option<SignedRecord>,
    #[serde(default)]
    pub(crate) revoked_at: Option<String>,
    #[serde(default)]
    pub(crate) revoke_reason: Option<String>,
}

impl MemberRow {
    pub(crate) fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct SessionRow {
    pub(crate) generation: u64,
    /// Highest sequence number assigned to a message towards this member.
    pub(crate) send_seq: u64,
    /// Highest sequence number processed from this member.
    pub(crate) recv_seq: u64,
    pub(crate) last_seen: String,
    #[serde(default)]
    pub(crate) tls: Option<String>,
    #[serde(default)]
    pub(crate) network: Option<String>,
    #[serde(default)]
    pub(crate) protocol: u32,
    #[serde(default)]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) rtt_ms: Option<u32>,
    #[serde(default)]
    pub(crate) inventory_version: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OutboxRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    pub(crate) env: Envelope,
    pub(crate) queued_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InviteRow {
    /// `sha256(admission_token)` hex.
    pub(crate) admission: String,
    pub(crate) wrapped_keys: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    pub(crate) exp: String,
    pub(crate) inviter: String,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SealedKeyRow {
    pub(crate) key: SealedKey,
    pub(crate) published_by: String,
    pub(crate) published_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InventoryRow {
    pub(crate) version: u64,
    pub(crate) envelope: Envelope,
    pub(crate) stored_at: String,
}

pub(crate) struct Store {
    db: Database,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Store")
    }
}

fn store_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("hub store: {context}: {error}")
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| store_error("cannot encode a row", error))
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| store_error("cannot decode a row", error))
}

fn get_json<T: DeserializeOwned>(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<Option<T>, String> {
    match table
        .get(key)
        .map_err(|error| store_error("cannot read a row", error))?
    {
        Some(guard) => decode(guard.value()).map(Some),
        None => Ok(None),
    }
}

fn get_u64(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<u64, String> {
    match table
        .get(key)
        .map_err(|error| store_error("cannot read a counter", error))?
    {
        Some(guard) => {
            let bytes = guard.value();
            let array = <[u8; 8]>::try_from(bytes)
                .map_err(|_| store_error("a counter has the wrong length", key))?;
            Ok(u64::from_le_bytes(array))
        }
        None => Ok(0),
    }
}

fn put_u64(
    table: &mut redb::Table<&'static str, &'static [u8]>,
    key: &str,
    value: u64,
) -> Result<(), String> {
    table
        .insert(key, value.to_le_bytes().as_slice())
        .map(|_| ())
        .map_err(|error| store_error("cannot write a counter", error))
}

impl Store {
    /// Opens or creates the database and every table.
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let db = Database::create(path).map_err(|error| {
            format!(
                "Cannot open the hub database {}: {error}",
                crate::paths::display_path(path)
            )
        })?;
        let store = Self { db };
        store.write(|txn| {
            txn.open_table(META).map_err(|error| store_error("cannot open meta", error))?;
            txn.open_table(MEMBERS).map_err(|error| store_error("cannot open members", error))?;
            txn.open_table(MEMBER_KEYS).map_err(|error| store_error("cannot open member_keys", error))?;
            txn.open_table(SESSIONS).map_err(|error| store_error("cannot open sessions", error))?;
            txn.open_table(OUTBOX).map_err(|error| store_error("cannot open outbox", error))?;
            txn.open_table(INVITES).map_err(|error| store_error("cannot open invites", error))?;
            txn.open_table(KEYS).map_err(|error| store_error("cannot open keys", error))?;
            txn.open_table(GRANTS).map_err(|error| store_error("cannot open grants", error))?;
            txn.open_table(INVENTORY).map_err(|error| store_error("cannot open inventory", error))?;
            txn.open_table(EVENTS).map_err(|error| store_error("cannot open events", error))?;
            let mut meta = txn.open_table(META).map_err(|error| store_error("cannot open meta", error))?;
            let format = get_u64(&meta, meta::FORMAT)?;
            if format == 0 {
                put_u64(&mut meta, meta::FORMAT, STORE_FORMAT)?;
            } else if format > STORE_FORMAT {
                return Err(format!(
                    "The hub database {} was written by a newer fastctx (format {format}); this build reads format {STORE_FORMAT} at most.",
                    crate::paths::display_path(path)
                ));
            }
            Ok(())
        })?;
        Ok(store)
    }

    fn write<R>(
        &self,
        f: impl FnOnce(&redb::WriteTransaction) -> Result<R, String>,
    ) -> Result<R, String> {
        let txn = self
            .db
            .begin_write()
            .map_err(|error| store_error("cannot begin a write", error))?;
        let result = f(&txn)?;
        txn.commit()
            .map_err(|error| store_error("cannot commit", error))?;
        Ok(result)
    }

    fn read<R>(
        &self,
        f: impl FnOnce(&redb::ReadTransaction) -> Result<R, String>,
    ) -> Result<R, String> {
        let txn = self
            .db
            .begin_read()
            .map_err(|error| store_error("cannot begin a read", error))?;
        f(&txn)
    }

    // ----- meta -----

    pub(crate) fn meta_string(&self, key: &str) -> Result<Option<String>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            match table
                .get(key)
                .map_err(|error| store_error("cannot read meta", error))?
            {
                Some(guard) => Ok(Some(String::from_utf8_lossy(guard.value()).into_owned())),
                None => Ok(None),
            }
        })
    }

    pub(crate) fn set_meta_string(&self, key: &str, value: &str) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            table
                .insert(key, value.as_bytes())
                .map(|_| ())
                .map_err(|error| store_error("cannot write meta", error))
        })
    }

    pub(crate) fn remove_meta(&self, key: &str) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            table
                .remove(key)
                .map(|_| ())
                .map_err(|error| store_error("cannot remove meta", error))
        })
    }

    pub(crate) fn meta_u64(&self, key: &str) -> Result<u64, String> {
        self.read(|txn| {
            let table = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            get_u64(&table, key)
        })
    }

    pub(crate) fn set_meta_u64(&self, key: &str, value: u64) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            put_u64(&mut table, key, value)
        })
    }

    // ----- members -----

    pub(crate) fn member(&self, name: &str) -> Result<Option<MemberRow>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            get_json(&table, name)
        })
    }

    pub(crate) fn member_by_key(&self, node_pub: &str) -> Result<Option<MemberRow>, String> {
        self.read(|txn| {
            let index = txn
                .open_table(MEMBER_KEYS)
                .map_err(|error| store_error("cannot open member_keys", error))?;
            let Some(name) = index
                .get(node_pub)
                .map_err(|error| store_error("cannot read member_keys", error))?
            else {
                return Ok(None);
            };
            let name = name.value().to_string();
            let table = txn
                .open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            get_json(&table, &name)
        })
    }

    pub(crate) fn members(&self) -> Result<Vec<MemberRow>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            let mut rows = Vec::new();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot iterate members", error))?
            {
                let (_, value) =
                    entry.map_err(|error| store_error("cannot iterate members", error))?;
                rows.push(decode::<MemberRow>(value.value())?);
            }
            Ok(rows)
        })
    }

    pub(crate) fn member_count(&self) -> Result<u64, String> {
        self.read(|txn| {
            let table = txn
                .open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            table
                .len()
                .map_err(|error| store_error("cannot count members", error))
        })
    }

    /// Inserts or replaces a member and bumps the members version, returning the new version.
    pub(crate) fn put_member(&self, row: &MemberRow) -> Result<u64, String> {
        let bytes = encode(row)?;
        self.write(|txn| {
            {
                let mut table = txn
                    .open_table(MEMBERS)
                    .map_err(|error| store_error("cannot open members", error))?;
                table
                    .insert(row.name.as_str(), bytes.as_slice())
                    .map_err(|error| store_error("cannot write a member", error))?;
                let mut index = txn
                    .open_table(MEMBER_KEYS)
                    .map_err(|error| store_error("cannot open member_keys", error))?;
                index
                    .insert(row.node_pub.as_str(), row.name.as_str())
                    .map_err(|error| store_error("cannot index a member", error))?;
            }
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            let version = get_u64(&meta, meta::MEMBERS_VERSION)? + 1;
            put_u64(&mut meta, meta::MEMBERS_VERSION, version)?;
            Ok(version)
        })
    }

    // ----- sessions -----

    pub(crate) fn session(&self, name: &str) -> Result<SessionRow, String> {
        self.read(|txn| {
            let table = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            Ok(get_json::<SessionRow>(&table, name)?.unwrap_or_default())
        })
    }

    pub(crate) fn sessions(&self) -> Result<BTreeMap<String, SessionRow>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            let mut rows = BTreeMap::new();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot iterate sessions", error))?
            {
                let (key, value) =
                    entry.map_err(|error| store_error("cannot iterate sessions", error))?;
                rows.insert(
                    key.value().to_string(),
                    decode::<SessionRow>(value.value())?,
                );
            }
            Ok(rows)
        })
    }

    /// Applies a change to one member's session row in a single transaction.
    pub(crate) fn update_session(
        &self,
        name: &str,
        change: impl FnOnce(&mut SessionRow),
    ) -> Result<SessionRow, String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            let mut row = get_json::<SessionRow>(&table, name)?.unwrap_or_default();
            change(&mut row);
            let bytes = encode(&row)?;
            table
                .insert(name, bytes.as_slice())
                .map_err(|error| store_error("cannot write a session", error))?;
            Ok(row)
        })
    }

    // ----- outbox (hub → member) -----

    /// Queues a reliable message towards `name`, assigning the next sequence number.
    pub(crate) fn outbox_push(
        &self,
        name: &str,
        env: &Envelope,
        id: Option<u64>,
        queued_at: &str,
    ) -> Result<u64, String> {
        self.write(|txn| {
            let mut sessions = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            let mut row = get_json::<SessionRow>(&sessions, name)?.unwrap_or_default();
            let mut outbox = txn
                .open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
            let depth = outbox
                .range::<(&str, u64)>((name, 0)..=(name, u64::MAX))
                .map_err(|error| store_error("cannot scan outbox", error))?
                .count() as u64;
            if depth >= OUTBOX_LIMIT {
                return Err(format!(
                    "outbox_full: {OUTBOX_LIMIT} messages are already waiting for \"{name}\"."
                ));
            }
            row.send_seq += 1;
            let seq = row.send_seq;
            let bytes = encode(&OutboxRow {
                id,
                env: env.clone(),
                queued_at: queued_at.to_string(),
            })?;
            outbox
                .insert((name, seq), bytes.as_slice())
                .map_err(|error| store_error("cannot queue a message", error))?;
            let session_bytes = encode(&row)?;
            sessions
                .insert(name, session_bytes.as_slice())
                .map_err(|error| store_error("cannot write a session", error))?;
            Ok(seq)
        })
    }

    /// Everything queued for `name` above `seq`, in order.
    pub(crate) fn outbox_after(
        &self,
        name: &str,
        seq: u64,
    ) -> Result<Vec<(u64, OutboxRow)>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
            let mut rows = Vec::new();
            for entry in table
                .range::<(&str, u64)>((name, seq.saturating_add(1))..=(name, u64::MAX))
                .map_err(|error| store_error("cannot scan outbox", error))?
            {
                let (key, value) =
                    entry.map_err(|error| store_error("cannot scan outbox", error))?;
                rows.push((key.value().1, decode::<OutboxRow>(value.value())?));
            }
            Ok(rows)
        })
    }

    /// Drops every queued message for `name` at or below `seq`.
    pub(crate) fn outbox_ack(&self, name: &str, seq: u64) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
            table
                .retain_in::<(&str, u64), _>((name, 0)..=(name, seq), |_, _| false)
                .map_err(|error| store_error("cannot ack outbox", error))?;
            Ok(())
        })
    }

    pub(crate) fn outbox_depth(&self, name: &str) -> Result<u64, String> {
        self.read(|txn| {
            let table = txn
                .open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
            Ok(table
                .range::<(&str, u64)>((name, 0)..=(name, u64::MAX))
                .map_err(|error| store_error("cannot scan outbox", error))?
                .count() as u64)
        })
    }

    pub(crate) fn outbox_clear(&self, name: &str) -> Result<(), String> {
        self.outbox_ack(name, u64::MAX)
    }

    // ----- invites -----

    pub(crate) fn invite(&self, code_id: &str) -> Result<Option<InviteRow>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            get_json(&table, code_id)
        })
    }

    pub(crate) fn put_invite(&self, code_id: &str, row: &InviteRow) -> Result<(), String> {
        let bytes = encode(row)?;
        self.write(|txn| {
            let mut table = txn
                .open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            table
                .insert(code_id, bytes.as_slice())
                .map(|_| ())
                .map_err(|error| store_error("cannot write an invite", error))
        })
    }

    pub(crate) fn remove_invite(&self, code_id: &str) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            table
                .remove(code_id)
                .map(|_| ())
                .map_err(|error| store_error("cannot remove an invite", error))
        })
    }

    /// Removes invites whose expiry is at or before `now`, returning how many were removed.
    pub(crate) fn expire_invites(&self, now: time::OffsetDateTime) -> Result<usize, String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            let mut removed = 0;
            table
                .retain(|_, value| {
                    let keep = decode::<InviteRow>(value)
                        .ok()
                        .and_then(|row| crate::world::parse_rfc3339(&row.exp).ok())
                        .is_some_and(|exp| exp > now);
                    if !keep {
                        removed += 1;
                    }
                    keep
                })
                .map_err(|error| store_error("cannot expire invites", error))?;
            Ok(removed)
        })
    }

    pub(crate) fn invite_count(&self) -> Result<u64, String> {
        self.read(|txn| {
            let table = txn
                .open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            table
                .len()
                .map_err(|error| store_error("cannot count invites", error))
        })
    }

    // ----- sealed keys -----

    pub(crate) fn put_sealed_key(
        &self,
        epoch: u32,
        name: &str,
        row: &SealedKeyRow,
    ) -> Result<(), String> {
        let bytes = encode(row)?;
        self.write(|txn| {
            let mut table = txn
                .open_table(KEYS)
                .map_err(|error| store_error("cannot open keys", error))?;
            table
                .insert((epoch, name), bytes.as_slice())
                .map(|_| ())
                .map_err(|error| store_error("cannot write a sealed key", error))
        })
    }

    /// Every sealed key addressed to `name`, ordered by epoch.
    pub(crate) fn sealed_keys_for(&self, name: &str) -> Result<Vec<(u32, SealedKeyRow)>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(KEYS)
                .map_err(|error| store_error("cannot open keys", error))?;
            let mut rows = Vec::new();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot iterate keys", error))?
            {
                let (key, value) =
                    entry.map_err(|error| store_error("cannot iterate keys", error))?;
                let (epoch, owner) = key.value();
                if owner == name {
                    rows.push((epoch, decode::<SealedKeyRow>(value.value())?));
                }
            }
            Ok(rows)
        })
    }

    pub(crate) fn remove_sealed_keys_for(&self, name: &str) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(KEYS)
                .map_err(|error| store_error("cannot open keys", error))?;
            table
                .retain(|(_, owner), _| owner != name)
                .map_err(|error| store_error("cannot remove sealed keys", error))?;
            Ok(())
        })
    }

    // ----- grants -----

    pub(crate) fn grants(&self) -> Result<Vec<SignedGrant>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(GRANTS)
                .map_err(|error| store_error("cannot open grants", error))?;
            let mut rows = Vec::new();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot iterate grants", error))?
            {
                let (_, value) =
                    entry.map_err(|error| store_error("cannot iterate grants", error))?;
                rows.push(decode::<SignedGrant>(value.value())?);
            }
            Ok(rows)
        })
    }

    /// Adds, replaces, or (with `None`) removes a grant, returning the new grant version.
    pub(crate) fn put_grant(&self, id: &str, grant: Option<&SignedGrant>) -> Result<u64, String> {
        let bytes = grant.map(encode).transpose()?;
        self.write(|txn| {
            {
                let mut table = txn
                    .open_table(GRANTS)
                    .map_err(|error| store_error("cannot open grants", error))?;
                match &bytes {
                    Some(bytes) => {
                        table
                            .insert(id, bytes.as_slice())
                            .map_err(|error| store_error("cannot write a grant", error))?;
                    }
                    None => {
                        table
                            .remove(id)
                            .map_err(|error| store_error("cannot remove a grant", error))?;
                    }
                }
            }
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            let version = get_u64(&meta, meta::GRANT_VERSION)? + 1;
            put_u64(&mut meta, meta::GRANT_VERSION, version)?;
            Ok(version)
        })
    }

    // ----- inventory -----

    pub(crate) fn inventories(&self) -> Result<Vec<(String, InventoryRow)>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(INVENTORY)
                .map_err(|error| store_error("cannot open inventory", error))?;
            let mut rows = Vec::new();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot iterate inventory", error))?
            {
                let (key, value) =
                    entry.map_err(|error| store_error("cannot iterate inventory", error))?;
                rows.push((
                    key.value().to_string(),
                    decode::<InventoryRow>(value.value())?,
                ));
            }
            Ok(rows)
        })
    }

    pub(crate) fn put_inventory(&self, name: &str, row: &InventoryRow) -> Result<(), String> {
        let bytes = encode(row)?;
        self.write(|txn| {
            {
                let mut table = txn
                    .open_table(INVENTORY)
                    .map_err(|error| store_error("cannot open inventory", error))?;
                table
                    .insert(name, bytes.as_slice())
                    .map_err(|error| store_error("cannot write inventory", error))?;
            }
            let mut sessions = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            let mut session = get_json::<SessionRow>(&sessions, name)?.unwrap_or_default();
            session.inventory_version = row.version;
            let session_bytes = encode(&session)?;
            sessions
                .insert(name, session_bytes.as_slice())
                .map_err(|error| store_error("cannot write a session", error))?;
            Ok(())
        })
    }

    pub(crate) fn remove_inventory(&self, name: &str) -> Result<(), String> {
        self.write(|txn| {
            let mut table = txn
                .open_table(INVENTORY)
                .map_err(|error| store_error("cannot open inventory", error))?;
            table
                .remove(name)
                .map(|_| ())
                .map_err(|error| store_error("cannot remove inventory", error))
        })
    }

    // ----- events -----

    /// Appends one event, assigning the next sequence number, and trims old history.
    pub(crate) fn append_event(
        &self,
        subject: &str,
        kind: &str,
        facts: BTreeMap<String, serde_json::Value>,
        task: Option<&str>,
    ) -> Result<Event, String> {
        self.write(|txn| {
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            let seq = get_u64(&meta, meta::EVENT_SEQ)? + 1;
            put_u64(&mut meta, meta::EVENT_SEQ, seq)?;
            let event = Event {
                seq,
                at: crate::world::now_rfc3339(),
                subject: subject.to_string(),
                kind: kind.to_string(),
                facts,
                task: task.map(str::to_string),
            };
            let bytes = encode(&event)?;
            let mut table = txn
                .open_table(EVENTS)
                .map_err(|error| store_error("cannot open events", error))?;
            table
                .insert(seq, bytes.as_slice())
                .map_err(|error| store_error("cannot write an event", error))?;
            if seq > EVENT_RETENTION {
                table
                    .retain_in(0..=(seq - EVENT_RETENTION), |_, _| false)
                    .map_err(|error| store_error("cannot trim events", error))?;
            }
            Ok(event)
        })
    }

    /// Events with `seq > since`, at most `limit`, plus the newest sequence number held.
    pub(crate) fn events_after(
        &self,
        since: u64,
        limit: usize,
    ) -> Result<(Vec<Event>, u64), String> {
        self.read(|txn| {
            let table = txn
                .open_table(EVENTS)
                .map_err(|error| store_error("cannot open events", error))?;
            let latest = table
                .last()
                .map_err(|error| store_error("cannot read the newest event", error))?
                .map(|(key, _)| key.value())
                .unwrap_or(0);
            let mut events = Vec::new();
            for entry in table
                .range(since.saturating_add(1)..)
                .map_err(|error| store_error("cannot scan events", error))?
                .take(limit)
            {
                let (_, value) = entry.map_err(|error| store_error("cannot scan events", error))?;
                events.push(decode::<Event>(value.value())?);
            }
            Ok((events, latest))
        })
    }

    pub(crate) fn event_count(&self) -> Result<u64, String> {
        self.read(|txn| {
            let table = txn
                .open_table(EVENTS)
                .map_err(|error| store_error("cannot open events", error))?;
            table
                .len()
                .map_err(|error| store_error("cannot count events", error))
        })
    }
}
