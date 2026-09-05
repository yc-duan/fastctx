//! `WorldClient`: the member's live view of its World and the one place that sends.
//!
//! The session task owns the socket; everything else (tool handlers, the executor, the admin
//! channel, status) goes through this handle: reliable sends land in the outbox first,
//! requests get a correlation id and a timeout, caches of members, grants, keys, and
//! inventories are refreshed from the hub and verified locally.
//!
//! Every fact that reaches this member through the hub is checked against something the hub
//! does not control before it is acted on: a member record against the World key and its
//! own signature, a revocation against a member already trusted, a rotated key against the
//! member that sealed it, a grant snapshot against its publisher and the revision already
//! held, and an answer against the id of the request it claims to answer.

use super::envelope::{Envelope, Header, Opened};
use super::grant::{self, GrantChange, GrantSet, GrantSnapshot};
use super::identity::Identity;
use super::keys::{KeyRing, SealedKey};
use super::members::{self, MemberTable, Selector, VerifiedMember};
use super::messages::{self, Call, CallBudget, CallResult, CallStatus, kind};
use super::outbox::{Outbox, OutboxEntry};
use super::state::{Counters, NodeState};
use super::wire::Frame;
use super::{HUB_NAME, NetworkMode, TlsMode, WorldConfig, WorldPaths};
use crate::model::ToolResponse;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// What a request collected before it returned.
pub(crate) struct Answers {
    /// Terminal answers: results, hub errors, and terminal call statuses.
    pub(crate) answers: Vec<Opened>,
    /// Targets the hub reported it handed the call to.
    pub(crate) delivered: BTreeSet<String>,
    /// The hub link dropped at least once while the request was waiting.
    pub(crate) link_lost: bool,
}

impl Answers {
    /// The one answer a hub request expects.
    fn single(self, what: &str) -> Result<Opened, String> {
        self.answers
            .into_iter()
            .next()
            .ok_or_else(|| format!("the hub did not answer {what}"))
    }
}

struct PendingRequest {
    tx: mpsc::UnboundedSender<Opened>,
    /// Hub-terminated requests fail as soon as the link drops; member-terminated calls keep
    /// waiting, because the hub delivers their answers on the next connection.
    to_hub: bool,
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
    /// The hub's last member listing, kept raw so the table can be re-verified when the key
    /// ring grows without another round trip.
    members_raw: RwLock<Option<messages::MembersResult>>,
    persistent: Mutex<Persistent>,
    outbox: Outbox,
    link: RwLock<LinkStatus>,
    sender: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    /// Bumped whenever the connection goes away, so a waiting request can tell.
    link_generation: AtomicU64,
    pub(crate) shutdown: CancellationToken,
    /// Poked to make the session act now (reconnect, publish).
    pub(crate) wake: Notify,
    /// Poked when this member learns a newer grant snapshot exists than the one it holds.
    pub(crate) grants_wanted: Notify,
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
            members_raw: RwLock::new(None),
            persistent: Mutex::new(Persistent { state, counters }),
            outbox,
            link: RwLock::new(link),
            sender: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            link_generation: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
            wake: Notify::new(),
            grants_wanted: Notify::new(),
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

    /// Forgets the connection. Hub-bound requests fail now; calls to members keep their
    /// place, because the hub answers them on whichever connection this member has next.
    pub(crate) fn detach_sender(&self) {
        *self.sender.lock() = None;
        self.link_generation.fetch_add(1, Ordering::SeqCst);
        let mut pending = self.pending.lock();
        pending.retain(|_, request| !request.to_hub);
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

    // ----- grants: revision, floor, staleness -----

    /// Revision of the grant snapshot in force here; 0 when none was ever received.
    pub(crate) fn grant_revision(&self) -> u64 {
        self.grants.read().revision
    }

    /// Whether a newer grant snapshot is known to exist than the one held. While true, calls
    /// from other members are refused as `grant_stale`: executing on an older set could honour
    /// a permission that has since been withdrawn.
    pub(crate) fn is_grant_stale(&self) -> bool {
        self.persistent.lock().state.grant_floor > self.grant_revision()
    }

    /// Records that a source the hub cannot edit says `revision` exists. Returns whether this
    /// member is now behind, in which case the caller wakes the session to fetch.
    pub(crate) fn raise_grant_floor(&self, revision: u64) -> bool {
        let raised = self
            .with_state(|state| {
                if revision > state.grant_floor {
                    state.grant_floor = revision;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        let behind = self.is_grant_stale();
        if raised && behind {
            self.grants_wanted.notify_one();
        }
        behind
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

    /// Sends a request expecting `expected` terminal answers within `timeout`; fails fast
    /// when the link is down. Hub status answers count, `delivered` notices do not: they are
    /// recorded so the caller can tell an undelivered leg from one whose outcome is unknown.
    pub(crate) async fn request<T: Serialize>(
        &self,
        header: Header,
        body: &T,
        encrypt: bool,
        sign: bool,
        expected: usize,
        timeout: Duration,
    ) -> Result<Answers, String> {
        if !self.is_connected() {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        let to_hub = header.to == HUB_NAME;
        let id = self.next_request_id()?;
        let env = self.build_envelope(header.with_id(id), body, encrypt, sign)?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.pending
            .lock()
            .insert(id, PendingRequest { tx, to_hub });
        let generation = self.link_generation.load(Ordering::SeqCst);
        if !self.send_frame(Frame::request(id, env)) {
            self.pending.lock().remove(&id);
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        let mut answers = Vec::with_capacity(expected);
        let mut delivered = BTreeSet::new();
        let deadline = tokio::time::Instant::now() + timeout;
        while answers.len() < expected {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(answer)) => match delivered_to(&answer) {
                    Some(node) => {
                        delivered.insert(node);
                    }
                    None => answers.push(answer),
                },
                Ok(None) => break,
                Err(_) => {
                    self.send_cancel(id);
                    break;
                }
            }
        }
        self.pending.lock().remove(&id);
        let link_lost = self.link_generation.load(Ordering::SeqCst) != generation;
        if to_hub && answers.is_empty() && (link_lost || !self.is_connected()) {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        Ok(Answers {
            answers,
            delivered,
            link_lost,
        })
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

    /// Hands an answer to whoever is waiting on `id`. The envelope's own `id` — inside the
    /// AEAD-authenticated header, so the hub cannot change it — must name the same request:
    /// the transport id alone is the hub's to assign, and a hub that could match answers to
    /// requests freely could hand one member's result to another member's question.
    pub(crate) fn deliver_answer(&self, id: u64, answer: Opened) -> Result<(), String> {
        if answer.header.id != Some(id) {
            return Err(format!(
                "answer {id} from \"{}\" names request {:?} in its header; dropped",
                answer.header.from, answer.header.id
            ));
        }
        match self.pending.lock().get(&id) {
            Some(pending) => pending
                .tx
                .send(answer)
                .map_err(|_| format!("request {id} is no longer waiting")),
            None => Err(format!("no request is waiting for answer {id}")),
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

    /// Fetches the hub's member listing and rebuilds the verified table on top of the one
    /// held. Also reseals the newest World key for listed members that lack it.
    pub(crate) async fn refresh_members(&self) -> Result<u64, String> {
        let header = Header::new(kind::MEMBERS_GET, &self.name(), HUB_NAME, 0);
        let answer = self
            .request(
                header,
                &serde_json::json!({}),
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind::MEMBERS_GET)?;
        expect_kind(&answer, kind::MEMBERS_RESULT)?;
        let result: messages::MembersResult = messages::decode(&answer.body, kind::MEMBERS_RESULT)?;
        let version = self.install_members(&result)?;
        *self.members_raw.write() = Some(result.clone());
        self.reseal_missing(&result);
        Ok(version)
    }

    fn install_members(&self, result: &messages::MembersResult) -> Result<u64, String> {
        let previous = self.members.read().clone();
        let table = {
            let keys = self.keys.read();
            MemberTable::from_entries(result.version, &result.members, &keys, &previous)
        };
        table.save(&self.paths)?;
        let version = table.version;
        *self.members.write() = table;
        self.with_state(|state| state.members_version = version)?;
        Ok(version)
    }

    /// Re-verifies the last listing with the current key ring; called after the ring grows,
    /// because records MAC'd under a newly adopted epoch verify only now.
    pub(crate) fn reverify_members(&self) -> Result<(), String> {
        let raw = self.members_raw.read().clone();
        if let Some(result) = raw {
            self.install_members(&result)?;
        }
        Ok(())
    }

    /// Seals the newest epoch for members the hub reports without a copy, when this member
    /// holds that epoch. Any holder may do this; the hub keeps the last copy per member.
    fn reseal_missing(&self, result: &messages::MembersResult) {
        if result.missing_key.is_empty() || result.key_epoch == 0 {
            return;
        }
        let me = self.name();
        let (epoch, sealed) = {
            let keys = self.keys.read();
            let current = keys.current();
            if current.epoch() != result.key_epoch {
                return;
            }
            let members = self.members.read();
            let mut sealed = Vec::new();
            for name in &result.missing_key {
                let Some(member) = members.get(name) else {
                    continue;
                };
                match member
                    .wrap_public()
                    .and_then(|wrap| SealedKey::seal(current, &wrap, &self.identity, &me))
                {
                    Ok(key) => sealed.push(messages::SealedKeyFor {
                        name: name.clone(),
                        key,
                    }),
                    Err(error) => super::node::log(format!(
                        "cannot seal the World key for \"{name}\": {error}"
                    )),
                }
            }
            (current.epoch(), sealed)
        };
        if sealed.is_empty() {
            return;
        }
        let names = sealed
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let header = Header::new(kind::KEY_PUBLISH, &me, HUB_NAME, 0);
        match self.send_reliable(header, &messages::KeyPublish { epoch, sealed }, false, true) {
            Ok(_) => super::node::log(format!("sealed World key epoch {epoch} for {names}")),
            Err(error) => super::node::log(format!("cannot publish sealed keys: {error}")),
        }
    }

    pub(crate) async fn refresh_inventories(&self) -> Result<usize, String> {
        let have = self
            .inventories
            .read()
            .iter()
            .map(|(name, (version, _))| (name.clone(), *version))
            .collect::<BTreeMap<_, _>>();
        let header = Header::new(kind::INVENTORY_GET, &self.name(), HUB_NAME, 0);
        let answer = self
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
            .await?
            .single(kind::INVENTORY_GET)?;
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
            if self.members.read().get(&entry.name).is_none() {
                continue;
            }
            if let Ok(inventory) = messages::decode::<Inventory>(&opened.body, kind::INVENTORY) {
                self.inventories
                    .write()
                    .insert(entry.name, (entry.version, inventory));
                updated += 1;
            }
        }
        Ok(updated)
    }

    /// Fetches the World key epochs this member lacks and adopts those a verified member
    /// sealed for it.
    ///
    /// Adoption is a fixed point: an epoch signed by a member whose record is MAC'd under a
    /// newer epoch verifies only after that newer epoch is held, so after every adoption the
    /// member table is re-verified and the remaining keys tried again.
    pub(crate) async fn refresh_keys(&self) -> Result<u32, String> {
        let have = self.keys.read().epochs();
        let header = Header::new(kind::KEYS_GET, &self.name(), HUB_NAME, 0);
        let answer = self
            .request(
                header,
                &messages::KeysGet { have },
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind::KEYS_GET)?;
        expect_kind(&answer, kind::KEYS_RESULT)?;
        let result: messages::KeysResult = messages::decode(&answer.body, kind::KEYS_RESULT)?;
        let my_wrap = self.identity.wrap_public();
        let mut remaining = result.sealed;
        remaining.sort_by_key(|sealed| sealed.epoch);
        let mut added = 0;
        loop {
            let mut progress = false;
            let mut kept = Vec::new();
            for sealed in remaining.drain(..) {
                if self.keys.read().get(sealed.epoch).is_some() {
                    continue;
                }
                let Some(publisher_key) = self.members.read().trusted_key(&sealed.published_by)
                else {
                    kept.push(sealed);
                    continue;
                };
                if let Err(error) = sealed.verify_publisher(&my_wrap, &publisher_key) {
                    super::node::log(format!("refusing a sealed World key: {error}"));
                    continue;
                }
                match sealed.open(&self.identity) {
                    Ok(key) => match self.keys.write().add(key) {
                        Ok(()) => {
                            added += 1;
                            progress = true;
                        }
                        Err(error) => super::node::log(format!(
                            "refusing a sealed World key from \"{}\": {error}",
                            sealed.published_by
                        )),
                    },
                    Err(error) => {
                        super::node::log(format!("cannot open a sealed World key: {error}"))
                    }
                }
            }
            remaining = kept;
            if !progress || remaining.is_empty() {
                break;
            }
            self.reverify_members()?;
        }
        if added > 0 {
            self.keys.read().save(&self.paths)?;
            self.reverify_members()?;
            let epoch = self.keys.read().current().epoch();
            self.update_link(|link| link.key_epoch = epoch);
            if self.is_grant_stale() {
                self.grants_wanted.notify_one();
            }
        }
        for sealed in &remaining {
            super::node::log(format!(
                "a sealed World key for epoch {} was not adopted: its publisher \"{}\" is not a verified member",
                sealed.epoch, sealed.published_by
            ));
        }
        let current = self.keys.read().current().epoch();
        if result.newest_epoch > current {
            return Err(format!(
                "key_epoch_unknown: the World is on key epoch {} but this member only has epoch {current}; no copy it can trust exists yet.",
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
        let answer = self
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
            .await?
            .single(kind::EVENTS_GET)?;
        expect_kind(&answer, kind::EVENTS_RESULT)?;
        messages::decode(&answer.body, kind::EVENTS_RESULT)
    }

    /// Asks the hub for the grant snapshot in force and applies it. Returns whether the set
    /// changed.
    ///
    /// The snapshot also arrives unasked, as a reliable broadcast. This is the repair path
    /// for the ways that can fail to land: a member whose outbox filled while it was away, a
    /// member that reconnects to a hub whose revision has moved on, and a member that learned
    /// from a peer's call that it is behind.
    pub(crate) async fn refresh_grants(&self) -> Result<bool, String> {
        let header = Header::new(kind::GRANTS_GET, &self.name(), HUB_NAME, 0);
        let answer = self
            .request(
                header,
                &serde_json::json!({}),
                false,
                false,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind::GRANTS_GET)?;
        expect_kind(&answer, kind::GRANT_SYNC)?;
        let sync: messages::GrantSync = messages::decode(&answer.body, kind::GRANT_SYNC)?;
        self.apply_grant_sync(sync)
    }

    /// Applies a `grant_sync` after verifying it, never moving backwards. Returns whether the
    /// set changed; every refusal is an error and leaves the set in force untouched.
    pub(crate) fn apply_grant_sync(&self, sync: messages::GrantSync) -> Result<bool, String> {
        let current = self.grants.read().clone();
        let Some(signed) = sync.set else {
            if current.revision == 0 {
                return Ok(false);
            }
            return Err(format!(
                "the hub reports no grant snapshot while revision {} is in force here; keeping it",
                current.revision
            ));
        };
        let snapshot = {
            let members = self.members.read();
            let keys = self.keys.read();
            grant::verify_snapshot(&signed, &keys, |name| members.trusted_key(name))?
        };
        if snapshot.revision < current.revision {
            return Err(format!(
                "the hub sent grant revision {} while revision {} is in force here; keeping it",
                snapshot.revision, current.revision
            ));
        }
        if snapshot.revision == current.revision {
            if current
                .signed
                .as_ref()
                .is_some_and(|mine| mine.snapshot == signed.snapshot)
            {
                return Ok(false);
            }
            return Err(format!(
                "the hub sent a different grant snapshot at revision {}; keeping the one in force",
                snapshot.revision
            ));
        }
        self.install_grants(snapshot, signed)?;
        Ok(true)
    }

    fn install_grants(
        &self,
        snapshot: GrantSnapshot,
        signed: grant::SignedGrantSet,
    ) -> Result<(), String> {
        let set = GrantSet::from_verified(snapshot, signed);
        set.save(&self.paths)?;
        let revision = set.revision;
        *self.grants.write() = set;
        self.with_state(|state| state.grant_revision = revision)?;
        Ok(())
    }

    /// Publishes the set in force plus `change` as the next revision. The hub refuses a
    /// revision that does not follow its own, in which case the set is refreshed and the
    /// change retried once on top of it.
    pub(crate) async fn change_grants(&self, change: GrantChange) -> Result<u64, String> {
        let me = self.name();
        for attempt in 0..2 {
            let current = self.grants.read().clone();
            let snapshot = GrantSnapshot {
                revision: current.revision + 1,
                grants: current.entries_after(&change)?,
                published_by: me.clone(),
                published_at: super::now_rfc3339(),
            };
            let signed = grant::sign_snapshot(&self.identity, &self.keys.read(), &snapshot)?;
            let header = Header::new(kind::GRANT_PUBLISH, &me, HUB_NAME, 0);
            let answer = self
                .request(
                    header,
                    &messages::GrantPublish {
                        set: signed.clone(),
                    },
                    false,
                    true,
                    1,
                    HUB_REQUEST_TIMEOUT,
                )
                .await?
                .single(kind::GRANT_PUBLISH)?;
            match answer.header.t.as_str() {
                kind::HUB_RESULT => {
                    let revision = snapshot.revision;
                    self.install_grants(snapshot, signed)?;
                    return Ok(revision);
                }
                kind::HUB_ERROR => {
                    let error: messages::HubError =
                        messages::decode(&answer.body, kind::HUB_ERROR)?;
                    if error.code == "grant_conflict" && attempt == 0 {
                        self.refresh_grants().await?;
                        continue;
                    }
                    return Err(format!("{}: {}", error.code, error.message));
                }
                other => {
                    return Err(format!(
                        "the hub answered {other} where hub_result was expected"
                    ));
                }
            }
        }
        Err(
            "The grant set changed twice while this change was being published; try again."
                .to_string(),
        )
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
        let signed = members::publish_record(&self.identity, &self.keys.read(), &record)?;
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
            wrapped_keys: invite.wrap_keys(&self.keys.read(), self.grant_revision())?,
            name,
            exp: invite.exp.clone(),
        };
        let header = Header::new(kind::INVITE_CREATE, &config.name, HUB_NAME, 0);
        self.send_reliable(header, &body, false, true)?;
        Ok(invite.encode())
    }

    /// Tells the hub this member is leaving, as a revocation signed by the member itself, so
    /// no later listing can bring the key back.
    pub(crate) fn leave(&self) -> Result<(), String> {
        // The outbox would take this happily and hold it forever: the enrollment is deleted
        // moments later and the daemon with it, so a leave queued on a link that is down never
        // reaches anyone. Say so instead of reporting that the hub was told.
        if !self.is_connected() {
            return Err(self.unreachable_error());
        }
        let me = self.name();
        let statement = messages::RevocationStatement {
            name: me.clone(),
            node_pub: super::crypto::b64_encode(&self.identity.public_key()),
            by: me.clone(),
            at: super::now_rfc3339(),
            reason: "left".to_string(),
        };
        let revocation = members::sign_revocation(&self.identity, &statement)?;
        let header = Header::new(kind::LEAVE, &me, HUB_NAME, 0);
        self.send_reliable(header, &messages::Revoke { revocation }, false, true)?;
        Ok(())
    }

    /// Sends a signed revocation of `name` and waits for the hub to record it.
    async fn send_revocation(&self, name: &str, reason: &str) -> Result<(), String> {
        let me = self.name();
        let node_pub = self
            .members
            .read()
            .identity(name)
            .map(|member| member.record.node_pub.clone())
            .ok_or_else(|| {
                format!("No verified member is named \"{name}\"; list machines with 'fastctx world nodes'.")
            })?;
        let statement = messages::RevocationStatement {
            name: name.to_string(),
            node_pub,
            by: me.clone(),
            at: super::now_rfc3339(),
            reason: reason.to_string(),
        };
        let revocation = members::sign_revocation(&self.identity, &statement)?;
        let header = Header::new(kind::REVOKE, &me, HUB_NAME, 0);
        let answer = self
            .request(
                header,
                &messages::Revoke { revocation },
                false,
                true,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind::REVOKE)?;
        expect_kind(&answer, kind::HUB_RESULT)
    }

    /// Revokes `name` with a signed statement, then rotates the World key for everyone who
    /// remains.
    pub(crate) async fn revoke(&self, name: &str) -> Result<u32, String> {
        if name == self.name() {
            return Err(
                "A member cannot revoke itself; run 'fastctx node unenroll' instead.".to_string(),
            );
        }
        self.send_revocation(name, "revoked").await?;
        self.complete_rotation().await
    }

    /// Creates the next key epoch and seals it to every remaining member.
    ///
    /// A revocation the hub operator made without a World key is countersigned first: the
    /// members left out of the new epoch should be out by a member's signature, which every
    /// other member can verify, not by the hub's word.
    pub(crate) async fn complete_rotation(&self) -> Result<u32, String> {
        self.refresh_members().await?;
        let unsigned = {
            let members = self.members.read();
            self.members_raw
                .read()
                .as_ref()
                .map(|result| {
                    result
                        .members
                        .iter()
                        .filter(|entry| {
                            entry.state == members::STATE_REVOKED
                                && entry.revocation.is_none()
                                && !members.revoked.contains_key(&entry.name)
                                && members.identity(&entry.name).is_some()
                        })
                        .map(|entry| entry.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        for name in &unsigned {
            match self.send_revocation(name, "revoked").await {
                Ok(()) => {
                    super::node::log(format!("countersigned the hub's revocation of \"{name}\""))
                }
                Err(error) => super::node::log(format!(
                    "cannot countersign the hub's revocation of \"{name}\": {error}"
                )),
            }
        }
        if !unsigned.is_empty() {
            self.refresh_members().await?;
        }
        let me = self.name();
        let members = self.members.read().clone();
        let (epoch, sealed) = {
            let mut keys = self.keys.write();
            let key = keys.rotate()?.clone();
            let mut sealed = Vec::new();
            for member in members
                .members
                .values()
                .filter(|member| member.is_current())
            {
                let wrap = member.wrap_public()?;
                sealed.push(messages::SealedKeyFor {
                    name: member.record.name.clone(),
                    key: SealedKey::seal(&key, &wrap, &self.identity, &me)?,
                });
            }
            keys.save(&self.paths)?;
            (key.epoch(), sealed)
        };
        let header = Header::new(kind::KEY_PUBLISH, &me, HUB_NAME, 0);
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
            .filter(|member| member.is_current())
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
        let remote = self.expand(selector)?;
        if remote.is_empty() {
            return Ok(Vec::new());
        }
        if !self.is_connected() {
            return Err(format!("hub_unreachable: {}", self.unreachable_error()));
        }
        // The hub routes a fan-out by `targets`; `to` carries the first of them so a hub that
        // understands only single-target routing still delivers one leg rather than none.
        let header = Header::new(kind::CALL, &me, &remote[0], 0)
            .with_verb(verb)
            .with_targets(remote.clone());
        let body = Call {
            verb: verb.to_string(),
            args,
            budget,
            cwd,
            timeout_ms: tool_timeout.as_millis() as u64,
            grant_revision: self.grant_revision(),
        };
        let window = tool_timeout + LINK_MARGIN;
        let Answers {
            answers,
            delivered,
            link_lost,
        } = self
            .request(header, &body, true, false, remote.len(), window)
            .await?;
        let mut outcomes: BTreeMap<String, NodeOutcome> = BTreeMap::new();
        for answer in answers {
            match answer.header.t.as_str() {
                kind::CALL_RESULT if answer.encrypted => {
                    let Ok(result) =
                        messages::decode::<CallResult>(&answer.body, kind::CALL_RESULT)
                    else {
                        continue;
                    };
                    // A result answers for the member that sent it, and only for one this call
                    // was addressed to; both fields sit inside the ciphertext.
                    if result.node != answer.header.from
                        || !remote.contains(&result.node)
                        || outcomes.contains_key(&result.node)
                    {
                        continue;
                    }
                    self.raise_grant_floor(result.grant_revision);
                    outcomes.insert(
                        result.node.clone(),
                        NodeOutcome {
                            node: result.node,
                            status: if result.response.is_error {
                                "error"
                            } else {
                                "ok"
                            }
                            .to_string(),
                            response: Some(result.response.into()),
                            message: None,
                        },
                    );
                }
                kind::CALL_STATUS if answer.header.from == HUB_NAME => {
                    let Ok(status) =
                        messages::decode::<CallStatus>(&answer.body, kind::CALL_STATUS)
                    else {
                        continue;
                    };
                    if !remote.contains(&status.node) || outcomes.contains_key(&status.node) {
                        continue;
                    }
                    outcomes.insert(
                        status.node.clone(),
                        NodeOutcome {
                            node: status.node,
                            status: status.status,
                            response: None,
                            message: Some(status.message),
                        },
                    );
                }
                _ => {}
            }
        }
        for target in &remote {
            if outcomes.contains_key(target) {
                continue;
            }
            let (status, message) = if !link_lost {
                (
                    "timeout",
                    format!(
                        "Node \"{target}\" did not answer within {} s.",
                        window.as_secs()
                    ),
                )
            } else if delivered.contains(target) {
                (
                    "unknown",
                    format!(
                        "The hub handed this call to \"{target}\" before the hub link dropped; no answer arrived within {} s, so it may have run.",
                        window.as_secs()
                    ),
                )
            } else {
                (
                    "unreachable",
                    format!(
                        "The hub link dropped before \"{target}\" received this call; nothing ran there."
                    ),
                )
            };
            outcomes.insert(
                target.clone(),
                NodeOutcome {
                    node: target.clone(),
                    status: status.to_string(),
                    response: None,
                    message: Some(message),
                },
            );
        }
        Ok(outcomes.into_values().collect())
    }
}

/// The target a `delivered` call status names, if the answer is one.
fn delivered_to(answer: &Opened) -> Option<String> {
    if answer.header.t != kind::CALL_STATUS || answer.header.from != HUB_NAME {
        return None;
    }
    let status: CallStatus = messages::decode(&answer.body, kind::CALL_STATUS).ok()?;
    (status.status == messages::CALL_DELIVERED).then_some(status.node)
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
