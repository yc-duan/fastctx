//! `WorldClient`: the member's live view of its World and the one place that sends.
//!
//! The session task owns the socket; everything else (tool handlers, the executor, the admin
//! channel, status) goes through this handle: reliable sends land in the outbox first,
//! requests get a correlation id and a timeout, caches of members, grants, keys, and
//! inventories are refreshed from the hub and verified locally.

use super::envelope::{Envelope, Header, Opened};
use super::grant::GrantSet;
use super::identity::Identity;
use super::keys::{KeyRing, SealedKey};
use super::members::{MemberTable, Selector, VerifiedMember};
use super::messages::{self, Call, CallBudget, CallResult, CallStatus, kind};
use super::outbox::{Outbox, OutboxEntry};
use super::state::{Counters, NodeState};
use super::wire::Frame;
use super::{HUB_NAME, NetworkMode, TlsMode, WorldConfig, WorldPaths};
use crate::model::ToolResponse;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

/// Default answer window for hub requests (`inventory_get`, `members_get`, `events_get`).
pub(crate) const HUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Added to a tool's own timeout for the round trip through the hub.
pub(crate) const LINK_MARGIN: Duration = Duration::from_secs(5);
/// Reliable message ack window before the link is treated as dead.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(15);

/// Facts a member publishes about itself, encrypted (`design-objects.md` §2 `facts`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Inventory {
    pub(crate) hostname: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) cpus: u32,
    pub(crate) memory_gb: f32,
    pub(crate) disks: Vec<Disk>,
    pub(crate) gpus: Vec<Gpu>,
    #[serde(default)]
    pub(crate) wsl2: Option<bool>,
    pub(crate) shell: Shell,
    pub(crate) capabilities: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) addresses: Vec<String>,
    pub(crate) version: String,
    pub(crate) collected_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Disk {
    pub(crate) mount: String,
    pub(crate) free_gb: f32,
    pub(crate) total_gb: f32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Gpu {
    pub(crate) index: u32,
    pub(crate) vendor: String,
    pub(crate) model: String,
    pub(crate) memory_gb: f32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Shell {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) login_ok: bool,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// Where the link is right now.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum LinkState {
    Starting,
    Connecting {
        attempt: u32,
    },
    Connected,
    Reconnecting {
        attempt: u32,
        next_attempt_at: String,
    },
    /// The link gave up until something changes (revoked, protocol mismatch, replaced too often).
    Stopped {
        reason: String,
        until: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LinkStatus {
    #[serde(flatten)]
    pub(crate) state: LinkState,
    pub(crate) network: Option<NetworkMode>,
    pub(crate) path: Option<String>,
    pub(crate) interface: Option<String>,
    pub(crate) tunnels: Vec<String>,
    pub(crate) tls: TlsMode,
    pub(crate) connected_since: Option<String>,
    pub(crate) last_heartbeat_at: Option<String>,
    pub(crate) last_ack_at: Option<String>,
    pub(crate) rtt_ms: Option<u32>,
    pub(crate) hub_time_offset_s: Option<i64>,
    pub(crate) last_error: Option<String>,
    pub(crate) replaced_count: u32,
    pub(crate) key_epoch: u32,
    pub(crate) outbox_depth: usize,
    #[serde(skip)]
    pub(crate) last_contact: Option<Instant>,
}

impl LinkStatus {
    pub(crate) fn is_connected(&self) -> bool {
        matches!(self.state, LinkState::Connected)
    }

    pub(crate) fn last_contact_ago(&self) -> Option<Duration> {
        self.last_contact.map(|instant| instant.elapsed())
    }
}

/// One member as the `nodes` tool and the CLI show it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeView {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) last_seen: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) tags: Vec<String>,
    pub(crate) version: String,
    pub(crate) hub_rtt_ms: Option<u32>,
    pub(crate) tls: Option<String>,
    pub(crate) network: Option<String>,
    pub(crate) inventory: Option<Inventory>,
    pub(crate) is_self: bool,
}

/// The outcome of a direct call on one target member.
#[derive(Clone, Debug)]
pub(crate) struct NodeOutcome {
    pub(crate) node: String,
    /// `ok`, `error`, `offline`, `unreachable`, `forbidden`, `unknown`, `revoked`, `timeout`.
    pub(crate) status: String,
    pub(crate) response: Option<ToolResponse>,
    pub(crate) message: Option<String>,
}

struct PendingRequest {
    tx: mpsc::UnboundedSender<Opened>,
}

struct Persistent {
    state: NodeState,
    counters: Counters,
}

pub(crate) struct WorldClient {
    pub(crate) paths: WorldPaths,
    pub(crate) config: RwLock<WorldConfig>,
    pub(crate) identity: Identity,
    pub(crate) keys: RwLock<KeyRing>,
    pub(crate) members: RwLock<MemberTable>,
    pub(crate) grants: RwLock<GrantSet>,
    pub(crate) inventories: RwLock<BTreeMap<String, (u64, Inventory)>>,
    pub(crate) own_inventory: RwLock<Option<Inventory>>,
    persistent: Mutex<Persistent>,
    outbox: Outbox,
    link: RwLock<LinkStatus>,
    sender: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    pub(crate) shutdown: CancellationToken,
    /// Poked to make the session act now (reconnect, publish).
    pub(crate) wake: Notify,
    pub(crate) started_at: String,
}

impl std::fmt::Debug for WorldClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "WorldClient({})", self.config.read().name)
    }
}

impl WorldClient {
    /// Loads an enrolled member's files. `Ok(None)` when the machine is not enrolled.
    pub(crate) fn open(paths: WorldPaths) -> Result<Option<Arc<Self>>, String> {
        let Some(config) = super::load_config(&paths)? else {
            return Ok(None);
        };
        let identity = Identity::load(&paths)?.ok_or_else(|| {
            format!(
                "not_enrolled: {} exists but the identity key is missing. Run 'fastctx node unenroll' and enroll again.",
                crate::paths::display_path(&paths.config)
            )
        })?;
        let keys = KeyRing::load(&paths)?.ok_or_else(|| {
            "not_enrolled: the World key file is missing. Run 'fastctx node unenroll' and enroll again.".to_string()
        })?;
        let members = MemberTable::load(&paths)?.unwrap_or_default();
        let grants = GrantSet::load(&paths)?.unwrap_or_default();
        let mut state = NodeState::load(&paths)?;
        let counters = Counters::resume(&mut state);
        state.save(&paths)?;
        let outbox = Outbox::new(&paths);
        let outbox_depth = outbox.depth()?;
        let link = LinkStatus {
            state: LinkState::Starting,
            network: state.last_network,
            path: None,
            interface: config.interface.clone(),
            tunnels: Vec::new(),
            tls: config.tls,
            connected_since: None,
            last_heartbeat_at: None,
            last_ack_at: None,
            rtt_ms: None,
            hub_time_offset_s: None,
            last_error: None,
            replaced_count: state.replaced_count,
            key_epoch: keys.current().epoch(),
            outbox_depth,
            last_contact: None,
        };
        Ok(Some(Arc::new(Self {
            paths,
            config: RwLock::new(config),
            identity,
            keys: RwLock::new(keys),
            members: RwLock::new(members),
            grants: RwLock::new(grants),
            inventories: RwLock::new(BTreeMap::new()),
            own_inventory: RwLock::new(None),
            persistent: Mutex::new(Persistent { state, counters }),
            outbox,
            link: RwLock::new(link),
            sender: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            wake: Notify::new(),
            started_at: super::now_rfc3339(),
        })))
    }

    pub(crate) fn name(&self) -> String {
        self.config.read().name.clone()
    }

    pub(crate) fn own_tags(&self) -> Vec<String> {
        self.members
            .read()
            .get(&self.name())
            .map(|member| member.record.tags.clone())
            .unwrap_or_default()
    }

    // ----- link status -----

    pub(crate) fn link(&self) -> LinkStatus {
        let mut status = self.link.read().clone();
        status.outbox_depth = self.outbox.depth().unwrap_or(status.outbox_depth);
        status.key_epoch = self.keys.read().current().epoch();
        status
    }

    pub(crate) fn update_link(&self, change: impl FnOnce(&mut LinkStatus)) {
        change(&mut self.link.write());
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.link.read().is_connected()
    }

    /// Installs the outbound channel of a freshly authenticated connection.
    pub(crate) fn attach_sender(&self, sender: mpsc::UnboundedSender<Frame>) {
        *self.sender.lock() = Some(sender);
    }

    pub(crate) fn detach_sender(&self) {
        *self.sender.lock() = None;
        let stale = std::mem::take(&mut *self.pending.lock());
        drop(stale);
    }

    pub(crate) fn send_frame(&self, frame: Frame) -> bool {
        match self.sender.lock().as_ref() {
            Some(sender) => sender.send(frame).is_ok(),
            None => false,
        }
    }

    /// Why a call cannot be made right now, in the words the tool surface shows.
    pub(crate) fn unreachable_error(&self) -> String {
        let status = self.link();
        match status.state {
            LinkState::Connected => "The World hub connection is not ready.".to_string(),
            LinkState::Stopped { reason, .. } => format!("The World hub link is stopped: {reason}"),
            _ => match status.last_contact_ago() {
                Some(ago) => format!(
                    "The World hub is unreachable (last contact {} s ago; reconnecting).",
                    ago.as_secs()
                ),
                None => "The World hub is unreachable (no contact since this node started; reconnecting).".to_string(),
            },
        }
    }

    // ----- persistent state -----

    pub(crate) fn with_state<R>(
        &self,
        change: impl FnOnce(&mut NodeState) -> R,
    ) -> Result<R, String> {
        let mut persistent = self.persistent.lock();
        let result = change(&mut persistent.state);
        persistent.state.save(&self.paths)?;
        Ok(result)
    }

    pub(crate) fn state_snapshot(&self) -> NodeState {
        self.persistent.lock().state.clone()
    }

    fn next_n(&self) -> Result<u64, String> {
        let mut persistent = self.persistent.lock();
        let Persistent { state, counters } = &mut *persistent;
        let (value, reserve) = counters.next_n(state);
        if reserve {
            state.save(&self.paths)?;
        }
        Ok(value)
    }

    fn next_request_id(&self) -> Result<u64, String> {
        let mut persistent = self.persistent.lock();
        let Persistent { state, counters } = &mut *persistent;
        let (value, reserve) = counters.next_request(state);
        if reserve {
            state.save(&self.paths)?;
        }
        Ok(value)
    }

    /// Accepts an envelope counter from `from` only if it is above everything seen so far.
    pub(crate) fn accept_counter(&self, from: &str, n: u64) -> Result<(), String> {
        if from == HUB_NAME {
            return Ok(());
        }
        let mut persistent = self.persistent.lock();
        let seen = persistent.state.seen.entry(from.to_string()).or_insert(0);
        if n <= *seen {
            return Err(format!(
                "replay: counter {n} from \"{from}\" is not above {}",
                *seen
            ));
        }
        *seen = n;
        persistent.state.save(&self.paths)
    }

    // ----- envelopes -----

    fn build_envelope<T: Serialize>(
        &self,
        mut header: Header,
        body: &T,
        encrypt: bool,
        sign: bool,
    ) -> Result<Envelope, String> {
        header.n = self.next_n()?;
        let bytes = messages::encode(body)?;
        let mut env = if encrypt {
            Envelope::seal(header, &bytes, &self.keys.read())?
        } else {
            Envelope::seal_plain(header, &bytes)?
        };
        if sign {
            env.sign(self.identity.signing())?;
        }
        Ok(env)
    }

    /// Queues a reliable message (outbox first) and pushes it when connected.
    pub(crate) fn send_reliable<T: Serialize>(
        &self,
        header: Header,
        body: &T,
        encrypt: bool,
        sign: bool,
    ) -> Result<u64, String> {
        let env = self.build_envelope(header, body, encrypt, sign)?;
        let seq = self.with_state(|state| {
            state.send_seq += 1;
            state.send_seq
        })?;
        self.outbox.push(
            seq,
            &OutboxEntry {
                id: None,
                env: env.clone(),
                queued_at: super::now_rfc3339(),
            },
        )?;
        self.send_frame(Frame::reliable(seq, env));
        Ok(seq)
    }

    /// Everything the hub has not acknowledged, for replay after a reconnect.
    pub(crate) fn outbox_after(&self, seq: u64) -> Result<Vec<(u64, OutboxEntry)>, String> {
        self.outbox.ack(seq)?;
        Ok(self
            .outbox
            .entries()?
            .into_iter()
            .filter(|(queued, _)| *queued > seq)
            .collect())
    }

    pub(crate) fn outbox_ack(&self, seq: u64) -> Result<(), String> {
        self.outbox.ack(seq)
    }

    /// Sends a request expecting `expected` answers within `timeout`; fails fast when the
    /// link is down. Answers are delivered as opened envelopes, hub status answers included.
    pub(crate) async fn request<T: Serialize>(
        &self,
        header: Header,
        body: &T,
        encrypt: bool,
        sign: bool,
        expected: usize,
        timeout: Duration,
    ) -> Result<Vec<Opened>, String> {
        if !self.is_connected() {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        let id = self.next_request_id()?;
        let env = self.build_envelope(header.with_id(id), body, encrypt, sign)?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.pending.lock().insert(id, PendingRequest { tx });
        if !self.send_frame(Frame::request(id, env)) {
            self.pending.lock().remove(&id);
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        let mut answers = Vec::with_capacity(expected);
        let deadline = tokio::time::Instant::now() + timeout;
        while answers.len() < expected {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(answer)) => answers.push(answer),
                Ok(None) => break,
                Err(_) => {
                    self.send_cancel(id);
                    break;
                }
            }
        }
        self.pending.lock().remove(&id);
        if answers.is_empty() && !self.is_connected() {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        Ok(answers)
    }

    fn send_cancel(&self, id: u64) {
        let header = Header::new(kind::CANCEL, &self.name(), HUB_NAME, 0).with_id(id);
        if let Ok(env) = self.build_envelope(header, &messages::HubResult::default(), false, false)
        {
            self.send_frame(Frame::Msg {
                seq: None,
                id: None,
                env,
            });
        }
    }

    /// Hands an answer to whoever is waiting on `id`.
    pub(crate) fn deliver_answer(&self, id: u64, answer: Opened) -> bool {
        match self.pending.lock().get(&id) {
            Some(pending) => pending.tx.send(answer).is_ok(),
            None => false,
        }
    }

    /// Answers a request that arrived from the hub with hub-side id `hub_id`.
    pub(crate) fn send_answer<T: Serialize>(
        &self,
        hub_id: u64,
        header: Header,
        body: &T,
        encrypt: bool,
    ) -> Result<(), String> {
        let env = self.build_envelope(header, body, encrypt, false)?;
        if self.send_frame(Frame::request(hub_id, env)) {
            Ok(())
        } else {
            Err("the hub link closed before the answer could be sent".to_string())
        }
    }

    // ----- hub-backed caches -----

    pub(crate) async fn refresh_members(&self) -> Result<u64, String> {
        let header = Header::new(kind::MEMBERS_GET, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &serde_json::json!({}),
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer members_get".to_string())?;
        expect_kind(&answer, kind::MEMBERS_RESULT)?;
        let result: messages::MembersResult = messages::decode(&answer.body, kind::MEMBERS_RESULT)?;
        let table = MemberTable::from_entries(result.version, &result.members, &self.keys.read());
        table.save(&self.paths)?;
        let version = table.version;
        *self.members.write() = table;
        self.with_state(|state| state.members_version = version)?;
        Ok(version)
    }

    pub(crate) async fn refresh_inventories(&self) -> Result<usize, String> {
        let have = self
            .inventories
            .read()
            .iter()
            .map(|(name, (version, _))| (name.clone(), *version))
            .collect::<BTreeMap<_, _>>();
        let header = Header::new(kind::INVENTORY_GET, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &messages::InventoryGet {
                    names: Vec::new(),
                    have,
                },
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer inventory_get".to_string())?;
        expect_kind(&answer, kind::INVENTORY_RESULT)?;
        let result: messages::InventoryResult =
            messages::decode(&answer.body, kind::INVENTORY_RESULT)?;
        let mut updated = 0;
        for entry in result.entries {
            let opened = match entry.envelope.open(Some(&self.keys.read())) {
                Ok(opened) if opened.encrypted && opened.header.from == entry.name => opened,
                Ok(_) => continue,
                Err(_) => continue,
            };
            if let Ok(inventory) = messages::decode::<Inventory>(&opened.body, kind::INVENTORY) {
                self.inventories
                    .write()
                    .insert(entry.name, (entry.version, inventory));
                updated += 1;
            }
        }
        Ok(updated)
    }

    pub(crate) async fn refresh_keys(&self) -> Result<u32, String> {
        let have = self.keys.read().epochs();
        let header = Header::new(kind::KEYS_GET, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &messages::KeysGet { have },
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer keys_get".to_string())?;
        expect_kind(&answer, kind::KEYS_RESULT)?;
        let result: messages::KeysResult = messages::decode(&answer.body, kind::KEYS_RESULT)?;
        let mut added = 0;
        {
            let mut keys = self.keys.write();
            for sealed in result.sealed {
                match sealed.open(&self.identity) {
                    Ok(key) => {
                        if keys.add(key).is_ok() {
                            added += 1;
                        }
                    }
                    Err(error) => {
                        super::node::log(format!("cannot open a sealed World key: {error}"))
                    }
                }
            }
            if added > 0 {
                keys.save(&self.paths)?;
            }
        }
        let current = self.keys.read().current().epoch();
        if result.newest_epoch > current {
            return Err(format!(
                "key_epoch_unknown: the World is on key epoch {} but this member only has epoch {current}; no sealed copy for it exists yet.",
                result.newest_epoch
            ));
        }
        Ok(current)
    }

    pub(crate) async fn fetch_events(
        &self,
        since: u64,
        limit: u32,
    ) -> Result<messages::EventsResult, String> {
        let header = Header::new(kind::EVENTS_GET, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &messages::EventsGet {
                    since,
                    limit: Some(limit),
                },
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer events_get".to_string())?;
        expect_kind(&answer, kind::EVENTS_RESULT)?;
        messages::decode(&answer.body, kind::EVENTS_RESULT)
    }

    /// Asks the hub for the grant set in force and applies it.
    ///
    /// Grants also arrive unasked, as a reliable broadcast. This is the repair path for the two
    /// ways that can fail to land: a member whose outbox filled while it was away, and a member
    /// that reconnects to a hub whose grant version has moved on. Without it a narrowed grant can
    /// stay unapplied on exactly the machine most likely to need it — the one that was gone.
    pub(crate) async fn refresh_grants(&self) -> Result<Vec<String>, String> {
        let header = Header::new(kind::GRANTS_GET, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &serde_json::json!({}),
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer grants_get".to_string())?;
        expect_kind(&answer, kind::GRANT_SYNC)?;
        let sync: messages::GrantSync = messages::decode(&answer.body, kind::GRANT_SYNC)?;
        self.apply_grant_sync(sync)
    }

    /// Applies a `grant_sync` from the hub after verifying every grant.
    pub(crate) fn apply_grant_sync(
        &self,
        sync: messages::GrantSync,
    ) -> Result<Vec<String>, String> {
        let members = self.members.read();
        let lookup = |name: &str| {
            members
                .get(name)
                .and_then(|member| member.public_key().ok())
        };
        let (set, rejected) =
            GrantSet::from_signed(sync.version, &sync.grants, &self.keys.read(), lookup);
        drop(members);
        set.save(&self.paths)?;
        *self.grants.write() = set;
        self.with_state(|state| state.grant_version = sync.version)?;
        Ok(rejected)
    }

    /// Publishes this member's own record; done on every connect so the hub always holds one.
    pub(crate) fn publish_record(&self, inventory: &Inventory) -> Result<(), String> {
        let config = self.config.read().clone();
        let record = messages::MemberRecord {
            name: config.name.clone(),
            node_pub: super::crypto::b64_encode(&self.identity.public_key()),
            wrap_pub: super::crypto::b64_encode(&self.identity.wrap_public()),
            tags: inventory.tags.clone(),
            kind: "stateful".to_string(),
            os: inventory.os.clone(),
            arch: inventory.arch.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            enrolled_at: config.enrolled_at.clone(),
        };
        let signed = super::members::publish_record(&self.identity, &self.keys.read(), &record)?;
        let header = Header::new(kind::MEMBER_PUBLISH, &config.name, HUB_NAME, 0);
        self.send_reliable(header, &messages::MemberPublish { signed }, false, true)?;
        Ok(())
    }

    /// Publishes the encrypted inventory as a reliable message.
    pub(crate) fn publish_inventory(&self, inventory: &Inventory) -> Result<(), String> {
        let header = Header::new(kind::INVENTORY, &self.name(), HUB_NAME, 0);
        self.send_reliable(header, inventory, true, false)?;
        *self.own_inventory.write() = Some(inventory.clone());
        self.with_state(|state| state.inventory_version += 1)?;
        Ok(())
    }

    /// Registers an invite with the hub and returns the pasteable string.
    pub(crate) fn create_invite(
        &self,
        name: Option<String>,
        ttl: time::Duration,
        hubs: Option<Vec<String>>,
    ) -> Result<String, String> {
        let config = self.config.read().clone();
        let hub_key = super::identity::Fingerprint::parse(&config.hub_key)?;
        let invite = super::invite::Invite::new(
            hubs.unwrap_or(config.hub.clone()),
            hub_key,
            name.clone(),
            ttl,
        )?;
        let body = messages::InviteCreate {
            code_id: invite.code_id(),
            admission: invite.admission(),
            wrapped_keys: invite.wrap_keys(&self.keys.read())?,
            name,
            exp: invite.exp.clone(),
        };
        let header = Header::new(kind::INVITE_CREATE, &config.name, HUB_NAME, 0);
        self.send_reliable(header, &body, false, true)?;
        Ok(invite.encode())
    }

    /// Publishes a grant as this member, or removes the grant with `id` when `grant` is `None`.
    /// The hub answers every member (this one included) with a `grant_sync`.
    pub(crate) fn publish_grant(
        &self,
        id: &str,
        grant: Option<&super::grant::Grant>,
    ) -> Result<(), String> {
        let me = self.name();
        let signed = match grant {
            Some(grant) => {
                super::grant::publish_grant(&self.identity, &self.keys.read(), &me, id, grant)?
            }
            None => messages::SignedGrant {
                id: id.to_string(),
                grant: String::new(),
                mac: String::new(),
                mac_epoch: 0,
                sig: String::new(),
                published_by: me.clone(),
            },
        };
        let header = Header::new(kind::GRANT_PUBLISH, &me, HUB_NAME, 0);
        self.send_reliable(
            header,
            &messages::GrantPublish {
                grant: signed,
                delete: grant.is_none(),
            },
            false,
            true,
        )?;
        Ok(())
    }

    /// Asks the hub to revoke `name`, then rotates the World key for everyone who remains.
    pub(crate) async fn revoke(&self, name: &str) -> Result<u32, String> {
        if name == self.name() {
            return Err(
                "A member cannot revoke itself; run 'fastctx node unenroll' instead.".to_string(),
            );
        }
        let header = Header::new(kind::REVOKE, &self.name(), HUB_NAME, 0);
        let answers = self
            .request(
                header,
                &messages::Revoke {
                    name: name.to_string(),
                },
                false,
                true,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| "the hub did not answer the revoke request".to_string())?;
        if answer.header.t == kind::HUB_ERROR {
            let error: messages::HubError = messages::decode(&answer.body, kind::HUB_ERROR)?;
            return Err(error.message);
        }
        self.complete_rotation().await
    }

    /// Creates the next key epoch and seals it to every remaining member.
    pub(crate) async fn complete_rotation(&self) -> Result<u32, String> {
        self.refresh_members().await?;
        let members = self.members.read().clone();
        let (epoch, sealed) = {
            let mut keys = self.keys.write();
            let key = keys.rotate()?.clone();
            let mut sealed = Vec::new();
            for member in members.members.values() {
                let wrap = member.wrap_public()?;
                sealed.push(messages::SealedKeyFor {
                    name: member.record.name.clone(),
                    key: SealedKey::seal(&key, &wrap)?,
                });
            }
            keys.save(&self.paths)?;
            (key.epoch(), sealed)
        };
        let header = Header::new(kind::KEY_PUBLISH, &self.name(), HUB_NAME, 0);
        self.send_reliable(header, &messages::KeyPublish { epoch, sealed }, false, true)?;
        self.update_link(|link| link.key_epoch = epoch);
        Ok(epoch)
    }

    // ----- views -----

    pub(crate) fn nodes(&self) -> Vec<NodeView> {
        let me = self.name();
        let members = self.members.read();
        let inventories = self.inventories.read();
        let own = self.own_inventory.read().clone();
        let mut views = members
            .members
            .values()
            .map(|member| {
                let inventory = if member.record.name == me {
                    own.clone()
                } else {
                    inventories
                        .get(&member.record.name)
                        .map(|(_, inventory)| inventory.clone())
                };
                NodeView {
                    name: member.record.name.clone(),
                    state: if member.record.name == me && self.is_connected() {
                        "online".to_string()
                    } else {
                        member.state.clone()
                    },
                    last_seen: member.last_seen.clone(),
                    os: member.record.os.clone(),
                    arch: member.record.arch.clone(),
                    tags: member.record.tags.clone(),
                    version: member.record.version.clone(),
                    hub_rtt_ms: member.hub_rtt_ms,
                    tls: member.tls.clone(),
                    network: member.network.clone(),
                    inventory,
                    is_self: member.record.name == me,
                }
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.name.cmp(&right.name));
        views
    }

    pub(crate) fn member(&self, name: &str) -> Option<VerifiedMember> {
        self.members.read().get(name).cloned()
    }

    pub(crate) fn expand(&self, selector: &Selector) -> Result<Vec<String>, String> {
        self.members.read().expand(selector)
    }

    // ----- direct calls -----

    /// Runs `verb` with `args` on every target named by the selector and returns one outcome
    /// per target. Targets that are offline, forbidden, or silent produce status outcomes.
    pub(crate) async fn call(
        &self,
        verb: &str,
        selector: &Selector,
        args: serde_json::Value,
        budget: CallBudget,
        cwd: Option<String>,
        tool_timeout: Duration,
    ) -> Result<Vec<NodeOutcome>, String> {
        let me = self.name();
        let targets = self.expand(selector)?;
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let mut outcomes = Vec::new();
        let mut remote = Vec::new();
        for target in targets {
            if target == me {
                remote.push(target);
                continue;
            }
            remote.push(target);
        }
        if !self.is_connected() {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        let header = Header::new(kind::CALL, &me, &remote[0], 0)
            .with_verb(verb)
            .with_targets(remote.clone());
        let body = Call {
            verb: verb.to_string(),
            args,
            budget,
            cwd,
            timeout_ms: tool_timeout.as_millis() as u64,
        };
        let answers = self
            .request(
                header,
                &body,
                true,
                false,
                remote.len(),
                tool_timeout + LINK_MARGIN,
            )
            .await?;
        let mut answered = std::collections::BTreeSet::new();
        for answer in answers {
            match answer.header.t.as_str() {
                kind::CALL_RESULT if answer.encrypted => {
                    if let Ok(result) =
                        messages::decode::<CallResult>(&answer.body, kind::CALL_RESULT)
                    {
                        if result.node != answer.header.from {
                            continue;
                        }
                        answered.insert(result.node.clone());
                        outcomes.push(NodeOutcome {
                            node: result.node,
                            status: if result.response.is_error {
                                "error"
                            } else {
                                "ok"
                            }
                            .to_string(),
                            response: Some(result.response.into()),
                            message: None,
                        });
                    }
                }
                kind::CALL_STATUS if answer.header.from == HUB_NAME => {
                    if let Ok(status) =
                        messages::decode::<CallStatus>(&answer.body, kind::CALL_STATUS)
                    {
                        answered.insert(status.node.clone());
                        outcomes.push(NodeOutcome {
                            node: status.node,
                            status: status.status,
                            response: None,
                            message: Some(status.message),
                        });
                    }
                }
                _ => {}
            }
        }
        for target in remote {
            if !answered.contains(&target) {
                outcomes.push(NodeOutcome {
                    node: target.clone(),
                    status: "timeout".to_string(),
                    response: None,
                    message: Some(format!(
                        "Node \"{target}\" did not answer within {} s.",
                        (tool_timeout + LINK_MARGIN).as_secs()
                    )),
                });
            }
        }
        outcomes.sort_by(|left, right| left.node.cmp(&right.node));
        Ok(outcomes)
    }
}

fn expect_kind(answer: &Opened, expected: &str) -> Result<(), String> {
    if answer.header.t == expected {
        return Ok(());
    }
    if answer.header.t == kind::HUB_ERROR {
        let error: messages::HubError = messages::decode(&answer.body, kind::HUB_ERROR)?;
        return Err(format!("{}: {}", error.code, error.message));
    }
    Err(format!(
        "the hub answered {} where {expected} was expected",
        answer.header.t
    ))
}
