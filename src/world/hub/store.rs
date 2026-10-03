//! Transactional hub storage for admitted identities, signed statements and durable queues.

use crate::world::envelope::Envelope;
use crate::world::grant::SignedGrantSet;
use crate::world::messages::{Event, SignedRecord, SignedRevocation};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[path = "ledger_store.rs"]
mod ledger_store;

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const MEMBERS: TableDefinition<&str, &[u8]> = TableDefinition::new("members");
const MEMBER_KEYS: TableDefinition<&str, &str> = TableDefinition::new("member_keys");
const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");
const OUTBOX: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("outbox");
const DELIVERY_IDS: TableDefinition<&str, &[u8]> = TableDefinition::new("delivery_ids");
const INVITES: TableDefinition<&str, &[u8]> = TableDefinition::new("invites");
const GRANTS: TableDefinition<&str, &[u8]> = TableDefinition::new("grants");
const INVENTORY: TableDefinition<&str, &[u8]> = TableDefinition::new("inventory");
const EVENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("events");

/// Protocol v2 database format; earlier protocol databases are rejected.
const STORE_FORMAT: u64 = 3;
/// The single row of the grants table.
const GRANT_SNAPSHOT_KEY: &str = "snapshot";
/// Events kept before the oldest are dropped.
pub(crate) const EVENT_RETENTION: u64 = 100_000;

pub(crate) mod meta {
    pub(crate) const FORMAT: &str = "format";
    pub(crate) const WORLD_ID: &str = "world_id";
    pub(crate) const BOOTSTRAP_ADMISSION: &str = "bootstrap_admission";
    pub(crate) const BOOTSTRAP_USED: &str = "bootstrap_used";
    pub(crate) const MEMBERS_VERSION: &str = "members_version";
    pub(crate) const GRANT_VERSION: &str = "grant_version";
    pub(crate) const EVENT_SEQ: &str = "event_seq";
    pub(crate) const HUB_N: &str = "hub_n";
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberRow {
    pub(crate) name: String,
    /// Ed25519 public key, base64.
    pub(crate) node_pub: String,
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
    /// The member-signed revocation, when a member (not the hub operator) revoked this one.
    #[serde(default)]
    pub(crate) revocation: Option<SignedRevocation>,
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
    pub(crate) endorsement: crate::world::admission::Endorsement,
    pub(crate) used: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InventoryRow {
    pub(crate) version: u64,
    pub(crate) envelope: Envelope,
    pub(crate) stored_at: String,
}

/// What a machine presents to be admitted (`Store::admit_member`).
pub(crate) enum Admission<'a> {
    /// Consume this invite in the same transaction as the insert.
    Invite(&'a str),
    /// Become the World's first member; refused once any member or a used bootstrap exists.
    Bootstrap,
}

/// The outcome of an atomic admission.
pub(crate) enum AdmissionOutcome {
    /// Inserted. The members version moves too, but nobody is told until the member publishes
    /// its signed record, so it stays inside.
    Admitted,
    /// The name belongs to a member with another key.
    NameTaken,
    /// The key is already enrolled under this other name.
    KeyEnrolledAs(String),
    /// The key, or the name it presents with its own key, was revoked; members never trust
    /// that key again, so readmitting it would only produce a member nobody answers.
    Revoked,
    /// The invite was consumed by another enrollment, or never existed.
    InviteGone,
    /// A bootstrap after the World already had its first member.
    NotFirst,
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
            ledger_store::initialize(txn)?;
            txn.open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            txn.open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            txn.open_table(MEMBER_KEYS)
                .map_err(|error| store_error("cannot open member_keys", error))?;
            txn.open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            txn.open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
            txn.open_table(DELIVERY_IDS)
                .map_err(|e| store_error("open delivery ids", e))?;
            txn.open_table(INVITES)
                .map_err(|error| store_error("cannot open invites", error))?;
            txn.open_table(GRANTS)
                .map_err(|error| store_error("cannot open grants", error))?;
            txn.open_table(INVENTORY)
                .map_err(|error| store_error("cannot open inventory", error))?;
            txn.open_table(EVENTS)
                .map_err(|error| store_error("cannot open events", error))?;
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            let format = get_u64(&meta, meta::FORMAT)?;
            if format != 0 && format != STORE_FORMAT {
                return Err("Protocol v2 requires a new hub database".into());
            }
            if format != STORE_FORMAT {
                put_u64(&mut meta, meta::FORMAT, STORE_FORMAT)?;
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

    /// Admits a member in one transaction: the name and key checks, the invite consumption or
    /// the bootstrap flag, and the insert commit together or not at all. Two enrollments racing
    /// on one invite, one name, or the bootstrap password therefore cannot both succeed; the
    /// separate read-check-write steps this replaces let both through (2026-09-05 review).
    pub(crate) fn admit_member(
        &self,
        row: &MemberRow,
        admission: Admission<'_>,
    ) -> Result<AdmissionOutcome, String> {
        let bytes = encode(row)?;
        self.write(|txn| {
            let mut members = txn
                .open_table(MEMBERS)
                .map_err(|error| store_error("cannot open members", error))?;
            let mut index = txn
                .open_table(MEMBER_KEYS)
                .map_err(|error| store_error("cannot open member_keys", error))?;
            if let Some(existing) = get_json::<MemberRow>(&members, &row.name)? {
                if existing.node_pub != row.node_pub {
                    return Ok(AdmissionOutcome::NameTaken);
                }
                if existing.is_revoked() {
                    return Ok(AdmissionOutcome::Revoked);
                }
            }
            let enrolled_as = index
                .get(row.node_pub.as_str())
                .map_err(|error| store_error("cannot read member_keys", error))?
                .map(|name| name.value().to_string());
            if let Some(name) = enrolled_as
                && name != row.name
            {
                return Ok(
                    match get_json::<MemberRow>(&members, &name)?
                        .is_some_and(|row| row.is_revoked())
                    {
                        true => AdmissionOutcome::Revoked,
                        false => AdmissionOutcome::KeyEnrolledAs(name),
                    },
                );
            }
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            match admission {
                Admission::Invite(code_id) => {
                    let mut invites = txn
                        .open_table(INVITES)
                        .map_err(|error| store_error("cannot open invites", error))?;
                    let Some(mut invite) = get_json::<InviteRow>(&invites, code_id)? else {
                        return Ok(AdmissionOutcome::InviteGone);
                    };
                    if invite.used
                        || crate::world::parse_rfc3339(&invite.endorsement.expires)?
                            <= time::OffsetDateTime::now_utc()
                    {
                        return Ok(AdmissionOutcome::InviteGone);
                    }
                    let inviter = get_json::<MemberRow>(&members, &invite.endorsement.inviter)?
                        .ok_or("Invitation author missing")?;
                    if inviter.is_revoked() {
                        return Ok(AdmissionOutcome::Revoked);
                    }
                    let signed = row.signed.as_ref().ok_or("Member has no admission proof")?;
                    signed.admission.verify(
                        &row.name,
                        &row.node_pub,
                        &crate::world::crypto::b64_array(&inviter.node_pub, "inviter")?,
                    )?;
                    invite.used = true;
                    invites
                        .insert(code_id, encode(&invite)?.as_slice())
                        .map_err(|e| store_error("consume invite", e))?;
                }
                Admission::Bootstrap => {
                    let used = meta
                        .get(meta::BOOTSTRAP_USED)
                        .map_err(|error| store_error("cannot read meta", error))?
                        .is_some();
                    let count = members
                        .len()
                        .map_err(|error| store_error("cannot count members", error))?;
                    if used || count > 0 {
                        return Ok(AdmissionOutcome::NotFirst);
                    }
                    if row.signed.as_ref().is_none_or(|signed| {
                        signed.admission != crate::world::admission::Credential::Founder
                    }) {
                        return Err("Bootstrap needs a founder record".into());
                    }
                    meta.insert("founder", row.node_pub.as_bytes())
                        .map_err(|e| store_error("pin founder", e))?;
                    meta.insert(meta::BOOTSTRAP_USED, b"1".as_slice())
                        .map_err(|error| store_error("cannot write meta", error))?;
                }
            }
            members
                .insert(row.name.as_str(), bytes.as_slice())
                .map_err(|error| store_error("cannot write a member", error))?;
            index
                .insert(row.node_pub.as_str(), row.name.as_str())
                .map_err(|error| store_error("cannot index a member", error))?;
            let version = get_u64(&meta, meta::MEMBERS_VERSION)? + 1;
            put_u64(&mut meta, meta::MEMBERS_VERSION, version)?;
            Ok(AdmissionOutcome::Admitted)
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
        crate::world::wire::Frame::reliable(u64::MAX, env.clone()).encode()?;
        let header = env.header()?;
        let letter = if header.t == crate::world::messages::kind::MAIL {
            Some(
                crate::world::messages::decode::<crate::world::mail::Letter>(
                    &env.body_bytes()?,
                    "mail",
                )?,
            )
        } else {
            None
        };
        let dedup = letter
            .as_ref()
            .map(|letter| format!("{}:{}:{}", header.from, name, letter.id));
        let hash = hex::encode(crate::world::crypto::sha256(&encode(env)?));
        self.write(|txn| {
            let mut ids = txn
                .open_table(DELIVERY_IDS)
                .map_err(|e| store_error("open delivery ids", e))?;
            if let Some(id) = &dedup
                && let Some((previous, seq)) = get_json::<(String, u64)>(&ids, id)?
            {
                if previous != hash {
                    return Err("mail id reused with different bytes".into());
                }
                return Ok(seq);
            }

            let mut sessions = txn
                .open_table(SESSIONS)
                .map_err(|error| store_error("cannot open sessions", error))?;
            let mut row = get_json::<SessionRow>(&sessions, name)?.unwrap_or_default();
            let mut outbox = txn
                .open_table(OUTBOX)
                .map_err(|error| store_error("cannot open outbox", error))?;
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
            if let Some(id) = &dedup {
                ids.insert(id.as_str(), encode(&(hash.clone(), seq))?.as_slice())
                    .map_err(|e| store_error("store delivery id", e))?;
            }
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
            if let Some(previous) = get_json::<InviteRow>(&table, code_id)? {
                return if previous.endorsement == row.endorsement {
                    Ok(())
                } else {
                    Err("Invitation public key already registered or consumed".into())
                };
            }
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
                    let keep = decode::<InviteRow>(value).ok().is_some_and(|row| {
                        row.used
                            || crate::world::parse_rfc3339(&row.endorsement.expires)
                                .is_ok_and(|exp| exp > now)
                    });
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
            let mut count = 0;
            let now = time::OffsetDateTime::now_utc();
            for entry in table
                .iter()
                .map_err(|error| store_error("cannot count invites", error))?
            {
                let (_, value) =
                    entry.map_err(|error| store_error("cannot count invites", error))?;
                let row: InviteRow = decode(value.value())?;
                if !row.used && crate::world::parse_rfc3339(&row.endorsement.expires)? > now {
                    count += 1;
                }
            }
            Ok(count)
        })
    }

    // ----- grants -----

    /// The signed grant snapshot in force; `None` when no member has published one.
    pub(crate) fn grant_snapshot(&self) -> Result<Option<SignedGrantSet>, String> {
        self.read(|txn| {
            let table = txn
                .open_table(GRANTS)
                .map_err(|error| store_error("cannot open grants", error))?;
            get_json(&table, GRANT_SNAPSHOT_KEY)
        })
    }

    /// Stores a snapshot whose revision must follow the stored one, in one transaction, so
    /// two members publishing at once cannot silently overwrite each other.
    pub(crate) fn put_grant_snapshot(
        &self,
        signed: &SignedGrantSet,
        revision: u64,
    ) -> Result<(), String> {
        let bytes = encode(signed)?;
        self.write(|txn| {
            let mut meta = txn
                .open_table(META)
                .map_err(|error| store_error("cannot open meta", error))?;
            let current = get_u64(&meta, meta::GRANT_VERSION)?;
            if revision != current + 1 {
                return Err(format!(
                    "grant_conflict: the hub holds grant revision {current}; a new snapshot must be revision {}, not {revision}.",
                    current + 1
                ));
            }
            let mut table = txn
                .open_table(GRANTS)
                .map_err(|error| store_error("cannot open grants", error))?;
            table
                .insert(GRANT_SNAPSHOT_KEY, bytes.as_slice())
                .map_err(|error| store_error("cannot write the grant snapshot", error))?;
            put_u64(&mut meta, meta::GRANT_VERSION, revision)?;
            Ok(())
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
