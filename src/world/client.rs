//! The shared member client: verified peers, signed messages, local durability and resident APIs.

use super::envelope::{Envelope, Header, Opened};
use super::grant::{self, GrantChange, GrantSet, GrantSnapshot};
use super::identity::Identity;
use super::members::{self, MemberTable, VerifiedMember};
use super::messages::{self, Call, CallBudget, CallOrigin, CallResult, CallStatus, kind};
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
/// Added to a tool's own timeout for the round trip through the hub and for the node's own
/// margin (`executor::VERB_TIMEOUT_MARGIN`): a command stopped at its timeout still reports.
pub(crate) const LINK_MARGIN: Duration = Duration::from_secs(15);
/// Reliable message ack window before the link is treated as dead.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(15);

/// Facts a member publishes about itself, signed.
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
    /// The home directory of the user the node runs as, in that machine's native spelling;
    /// where a remote `run` without `cwd` starts.
    #[serde(default)]
    pub(crate) home: String,
    /// The user the node runs as.
    #[serde(default)]
    pub(crate) user: String,
    /// User-authored description from this machine's local profile, signed with inventory.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    /// Last observed runtime usage; old nodes omit this rather than claiming zero activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) activity: Option<super::node::activity::NodeActivity>,
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
    /// Controller-local presentation metadata; never published in inventory or other frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) private_note: Option<String>,
}

impl NodeView {
    /// One effective label for compact local surfaces; the TUI also displays both sources.
    pub(crate) fn effective_note(&self) -> Option<&str> {
        self.private_note
            .as_deref()
            .filter(|note| !note.is_empty())
            .or_else(|| {
                self.inventory
                    .as_ref()
                    .map(|inventory| inventory.description.as_str())
                    .filter(|note| !note.is_empty())
            })
    }
}

/// The outcome of a direct call on one target member.
#[derive(Clone, Debug)]
pub(crate) struct NodeOutcome {
    pub(crate) node: String,
    /// `ok`, `error`, `offline`, `unreachable`, `forbidden`, `unknown`, `revoked`, `timeout`.
    pub(crate) status: String,
    pub(crate) response: Option<ToolResponse>,
    pub(crate) message: Option<String>,
    /// Structured facts the target attached (`CallResult::facts`).
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
}

impl NodeOutcome {
    /// A terminal status the hub or this member decided without the target answering.
    pub(crate) fn status(node: &str, status: &str, message: String) -> Self {
        Self {
            node: node.to_string(),
            status: status.to_string(),
            response: None,
            message: Some(message),
            facts: BTreeMap::new(),
        }
    }
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
    target: String,
    tx: mpsc::UnboundedSender<Opened>,
    /// Hub-terminated requests fail as soon as the link drops; member-terminated calls keep
    /// waiting, because the hub delivers their answers on the next connection.
    to_hub: bool,
}

struct RequestLifetime<'a> {
    client: &'a WorldClient,
    id: u64,
}
impl Drop for RequestLifetime<'_> {
    fn drop(&mut self) {
        if self.client.pending.lock().remove(&self.id).is_some() {
            self.client.send_cancel(self.id);
        }
    }
}

struct SubscriptionAttempt<'a> {
    client: &'a WorldClient,
    id: String,
    accepted: bool,
}
impl Drop for SubscriptionAttempt<'_> {
    fn drop(&mut self) {
        if !self.accepted {
            let active = self
                .client
                .subscriptions
                .active
                .lock()
                .contains_key(&self.id);
            if active && let Err(error) = self.client.unsubscribe(&self.id) {
                super::node::log(format!("subscription cancellation: {error}"));
            }
        }
    }
}

struct Persistent {
    state: NodeState,
    counters: Counters,
}

pub(crate) struct WorldClient {
    pub(crate) paths: WorldPaths,
    pub(crate) config: RwLock<WorldConfig>,
    pub(crate) identity: Identity,
    pub(crate) hub_public: RwLock<Option<[u8; 32]>>,
    pub(crate) members: RwLock<MemberTable>,
    pub(crate) grants: RwLock<GrantSet>,
    pub(crate) inventories: RwLock<BTreeMap<String, (u64, Inventory)>>,
    pub(crate) own_inventory: RwLock<Option<Inventory>>,
    /// Last failed collection or publish, exposed to local health views until recovery.
    pub(crate) inventory_error: RwLock<Option<String>>,
    /// Weak ownership follows the control center's actual lifetime, not the daemon startup.
    pub(crate) runtime_activity: RwLock<std::sync::Weak<crate::runtime::activity::RuntimeActivity>>,
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
    pub(crate) inbox_changed: Notify,
    pub(crate) subscriptions: super::subscription::Subscriptions,
    pub(crate) inbox_lock: Mutex<()>,
    ledger_sync: tokio::sync::Mutex<()>,
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
        let members = MemberTable::load(&paths)?.unwrap_or_default();
        let grants = GrantSet::load(&paths)?.unwrap_or_default();
        let mut state = NodeState::load(&paths)?;
        state.send_seq = state.send_seq.max(
            Outbox::new(&paths)?
                .entries()?
                .last()
                .map_or(0, |(seq, _)| *seq),
        );
        let counters = Counters::resume(&mut state);
        state.save(&paths)?;
        let outbox = Outbox::new(&paths)?;
        let outbox_depth = outbox.depth();
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
            outbox_depth,
            last_contact: None,
        };
        Ok(Some(Arc::new(Self {
            paths,
            config: RwLock::new(config),
            identity,
            hub_public: RwLock::new(None),
            members: RwLock::new(members),
            grants: RwLock::new(grants),
            inventories: RwLock::new(BTreeMap::new()),
            own_inventory: RwLock::new(None),
            inventory_error: RwLock::new(None),
            runtime_activity: RwLock::new(std::sync::Weak::new()),
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
            inbox_changed: Notify::new(),
            subscriptions: Default::default(),
            inbox_lock: Mutex::new(()),
            ledger_sync: tokio::sync::Mutex::new(()),
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
        status.outbox_depth = self.outbox.depth();
        status
    }

    pub(crate) fn update_link(&self, change: impl FnOnce(&mut LinkStatus)) {
        change(&mut self.link.write());
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.link.read().is_connected()
    }

    /// Installs the outbound channel of a freshly authenticated connection.
    pub(crate) fn attach_sender(
        &self,
        sender: mpsc::UnboundedSender<Frame>,
        acknowledged: u64,
    ) -> Result<(), String> {
        let _persistent = self.persistent.lock();
        for (seq, entry) in self.outbox_after(acknowledged)? {
            sender
                .send(Frame::reliable(seq, entry.env))
                .map_err(|e| e.to_string())?;
        }
        *self.sender.lock() = Some(sender);
        Ok(())
    }

    /// Forgets the connection. Hub-bound requests fail now; calls to members keep their
    /// place, because the hub answers them on whichever connection this member has next.
    pub(crate) fn detach_sender(&self) {
        *self.sender.lock() = None;
        self.subscriptions.disconnect();
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

    /// Accepts a counter once, including messages reordered across concurrent requests.
    pub(crate) fn accept_counter(&self, from: &str, n: u64) -> Result<(), String> {
        if from == HUB_NAME {
            return Ok(());
        }
        let mut persistent = self.persistent.lock();
        let seen = persistent.state.seen.entry(from.to_string()).or_default();
        if !seen.insert(n) {
            return Err(format!("replay: counter {n} from {from}"));
        }
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
    pub(crate) fn raise_grant_floor(&self, revision: u64) -> Result<bool, String> {
        let raised = self.with_state(|state| {
            if revision > state.grant_floor {
                state.grant_floor = revision;
                true
            } else {
                false
            }
        })?;
        let behind = self.is_grant_stale();
        if raised && behind {
            self.grants_wanted.notify_one();
        }
        Ok(behind)
    }

    /// Verifies every envelope against the locally pinned hub or admitted member key.
    pub(crate) fn open_envelope(&self, env: &Envelope) -> Result<Opened, String> {
        let header = env.header()?;
        let key = if header.from == HUB_NAME {
            self.hub_public.read().ok_or("Hub has not authenticated")?
        } else {
            self.members
                .read()
                .trusted_key(&header.from)
                .ok_or_else(|| format!("untrusted or revoked author {}", header.from))?
        };
        env.verify_signature(&key)?;
        env.open()
    }

    // ----- envelopes -----

    fn build_envelope<T: Serialize>(
        &self,
        mut header: Header,
        body: &T,
    ) -> Result<Envelope, String> {
        header.n = self.next_n()?;
        let bytes = messages::encode(body)?;
        let mut env = Envelope::seal_plain(header, &bytes)?;
        env.sign(self.identity.signing())?;
        Ok(env)
    }

    /// Queues a reliable message (outbox first) and pushes it when connected.
    pub(crate) fn send_reliable<T: Serialize>(
        &self,
        header: Header,
        body: &T,
    ) -> Result<u64, String> {
        let env = self.build_envelope(header, body)?;
        // Queue bytes before committing their sequence; recovery takes the maximum of both.
        let mut persistent = self.persistent.lock();
        let seq = persistent.state.send_seq + 1;
        Frame::reliable(u64::MAX, env.clone()).encode()?;
        self.outbox.push(
            seq,
            &OutboxEntry {
                id: None,
                env: env.clone(),
                queued_at: super::now_rfc3339(),
            },
        )?;
        persistent.state.send_seq = seq;
        persistent.state.save(&self.paths)?;
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

    pub(crate) fn record_rejection(
        &self,
        rejection: &super::delivery::Rejection,
    ) -> Result<(), String> {
        if let Some((_, entry)) = self
            .outbox
            .entries()?
            .into_iter()
            .find(|(seq, _)| *seq == rejection.seq)
        {
            let hash = hex::encode(super::crypto::sha256(&messages::encode(&entry.env)?));
            if hash != rejection.hash {
                return Err("Delivery rejection names different envelope bytes".into());
            }
        }
        super::delivery::record_outbound(&self.paths, rejection)
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
        expected: usize,
        timeout: Duration,
    ) -> Result<Answers, String> {
        if !self.is_connected() {
            return Err(self.unreachable_error());
        }
        let to_hub = header.to == HUB_NAME;
        let target = header.to.clone();
        let id = self.next_request_id()?;
        let env = self.build_envelope(header.with_id(id), body)?;
        Frame::request(u64::MAX, env.clone()).encode()?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.pending
            .lock()
            .insert(id, PendingRequest { tx, to_hub, target });
        let _lifetime = RequestLifetime { client: self, id };
        let generation = self.link_generation.load(Ordering::SeqCst);
        if !self.send_frame(Frame::request(id, env)) {
            self.pending.lock().remove(&id);
            return Err(self.unreachable_error());
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
            return Err(self.unreachable_error());
        }
        Ok(Answers {
            answers,
            delivered,
            link_lost,
        })
    }

    fn send_cancel(&self, id: u64) {
        let header = Header::new(kind::CANCEL, &self.name(), HUB_NAME, 0).with_id(id);
        if let Ok(env) = self.build_envelope(header, &messages::HubResult::default()) {
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
        let pending = self.pending.lock();
        let request = pending
            .get(&id)
            .ok_or_else(|| format!("no request is waiting for answer {id}"))?;
        let hub_reply = answer.header.from == HUB_NAME
            && (request.to_hub
                || matches!(
                    answer.header.t.as_str(),
                    kind::CALL_STATUS | kind::HUB_ERROR | kind::HUB_RESULT
                ));
        let peer_reply = answer.header.from == request.target
            && matches!(
                answer.header.t.as_str(),
                kind::CALL_RESULT | kind::CALL_RESULT_STREAM
            );
        if !hub_reply && !peer_reply {
            return Err("Answer author/type does not match pending request".into());
        }
        request
            .tx
            .send(answer)
            .map_err(|_| format!("request {id} is no longer waiting"))
    }

    /// Answers a request that arrived from the hub with hub-side id `hub_id`.
    pub(crate) fn send_answer(
        self: &Arc<Self>,
        hub_id: u64,
        mut header: Header,
        body: &CallResult,
        verb: &str,
        window: Duration,
    ) -> Result<(), String> {
        let env = self.build_envelope(header.clone(), body)?;
        let frame = Frame::request(hub_id, env);
        if frame.encode().is_ok() {
            if self.send_frame(frame) {
                return Ok(());
            }
            return Err(self.unreachable_error());
        }
        let bytes = messages::encode(body)?;
        let ticket = super::stream::ticket(self, &header.to, &header.to, &self.name(), verb)?;
        let description = super::result_stream::Description {
            bytes: bytes.len() as u64,
            sha256: hex::encode(super::crypto::sha256(&bytes)),
            ticket: ticket.clone(),
        };
        header.t = kind::CALL_RESULT_STREAM.into();
        let env = self.build_envelope(header, &description)?;
        let frame = Frame::request(hub_id, env);
        frame.encode()?;
        let client = Arc::clone(self);
        tokio::spawn(async move {
            let result = tokio::select! {
                result = tokio::time::timeout(window, super::result_stream::send(&client, ticket, bytes)) => result.unwrap_or_else(|_| Err("caller result deadline expired".into())),
                () = client.shutdown.cancelled() => Err("node stopped".into()),
            };
            if let Err(error) = result {
                super::node::log(format!("result stream failed: {error}"));
            }
        });
        if self.send_frame(frame) {
            Ok(())
        } else {
            Err(self.unreachable_error())
        }
    }

    // ----- hub-backed caches -----

    /// Fetches the hub's member listing and rebuilds the verified table on top of the one
    /// held, retaining historical admissions and permanent revocations.
    pub(crate) async fn refresh_members(&self) -> Result<u64, String> {
        let header = Header::new(kind::MEMBERS_GET, &self.name(), HUB_NAME, 0);
        let answer = self
            .request(header, &serde_json::json!({}), 1, HUB_REQUEST_TIMEOUT)
            .await?
            .single(kind::MEMBERS_GET)?;
        expect_kind(&answer, kind::MEMBERS_RESULT)?;
        let result: messages::MembersResult = messages::decode(&answer.body, kind::MEMBERS_RESULT)?;
        let version = self.install_members(&result)?;
        *self.members_raw.write() = Some(result.clone());
        Ok(version)
    }

    fn install_members(&self, result: &messages::MembersResult) -> Result<u64, String> {
        let mut members = self.members.write();
        let previous = members.clone();
        let table = {
            let config = self.config.read();
            MemberTable::from_entries(
                result.version,
                &result.members,
                &config.founder,
                &config.world_id,
                &previous,
            )
        };
        table.save(&self.paths)?;
        let version = table.version;
        *members = table;
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
        let answer = self
            .request(
                header,
                &messages::InventoryGet {
                    names: Vec::new(),
                    have,
                },
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
            let opened = match self.open_envelope(&entry.envelope) {
                Ok(opened) if opened.header.from == entry.name => opened,
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
            .request(header, &serde_json::json!({}), 1, HUB_REQUEST_TIMEOUT)
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
            grant::verify_snapshot(&signed, |name| members.trusted_key(name))?
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
        let mut current = self.grants.write();
        if snapshot.revision < current.revision {
            return Err("Grant revision would roll back local state".into());
        }
        if snapshot.revision == current.revision
            && current
                .signed
                .as_ref()
                .is_some_and(|known| known != &signed)
        {
            return Err("Grant snapshot conflicts at the same revision".into());
        }
        let set = GrantSet::from_verified(snapshot, signed);
        set.save(&self.paths)?;
        let revision = set.revision;
        *current = set;
        drop(current);
        self.with_state(|state| state.grant_revision = revision)?;
        Ok(())
    }

    /// Publishes the set in force plus `change` as the next revision. The hub refuses a
    /// revision that does not follow its own, in which case the set is refreshed and the
    /// change retried once on top of it.
    /// A reviewed revision is a compare-and-swap precondition, including after hub conflict.
    pub(crate) async fn change_grants_at(
        &self,
        change: GrantChange,
        expected_revision: Option<u64>,
    ) -> Result<u64, String> {
        let me = self.name();
        for attempt in 0..2 {
            let current = self.grants.read().clone();
            if expected_revision.is_some_and(|expected| expected != current.revision) {
                return Err("The grant snapshot changed after the preview. Refresh it and review the change again.".to_string());
            }
            let snapshot = GrantSnapshot {
                revision: current.revision + 1,
                grants: current.entries_after(&change)?,
                published_by: me.clone(),
                published_at: super::now_rfc3339(),
            };
            let signed = grant::sign_snapshot(&self.identity, &snapshot)?;
            let header = Header::new(kind::GRANT_PUBLISH, &me, HUB_NAME, 0);
            let answer = self
                .request(
                    header,
                    &messages::GrantPublish {
                        set: signed.clone(),
                    },
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
            tags: inventory.tags.clone(),
            kind: "stateful".to_string(),
            os: inventory.os.clone(),
            arch: inventory.arch.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            enrolled_at: config.enrolled_at.clone(),
        };
        let signed = members::publish_record(&self.identity, &config.admission, &record)?;
        let header = Header::new(kind::MEMBER_PUBLISH, &config.name, HUB_NAME, 0);
        self.send_reliable(header, &messages::MemberPublish { signed })?;
        Ok(())
    }

    /// Publishes the signed inventory as a reliable message.
    pub(crate) fn publish_inventory(&self, inventory: &Inventory) -> Result<(), String> {
        let mut inventory = inventory.clone();
        if inventory.activity.is_none() {
            inventory.activity = self
                .own_inventory
                .read()
                .as_ref()
                .and_then(|previous| previous.activity.clone());
        }
        // A slow hardware probe may have begun before a user saved a newer description.
        inventory.description = super::notes::description(&self.paths)?;
        let header = Header::new(kind::INVENTORY, &self.name(), HUB_NAME, 0);
        self.send_reliable(header, &inventory)?;
        *self.own_inventory.write() = Some(inventory);
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
        let invite = super::invite::Invite::new(
            &config,
            &self.identity,
            name,
            ttl,
            self.grant_revision().max(self.state_snapshot().grant_floor),
            hubs.unwrap_or(config.hub.clone()),
        )?;
        let body = messages::InviteCreate {
            endorsement: invite.endorsement.clone(),
        };
        let header = Header::new(kind::INVITE_CREATE, &config.name, HUB_NAME, 0);
        self.send_reliable(header, &body)?;
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
        self.send_reliable(header, &messages::Revoke { revocation })?;
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
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind::REVOKE)?;
        expect_kind(&answer, kind::HUB_RESULT)
    }

    /// Revokes a device with a member signature; descendants retain their admissions.
    pub(crate) async fn revoke(&self, name: &str) -> Result<(), String> {
        if name == self.name() {
            return Err(
                "A member cannot revoke itself; run 'fastctx node unenroll' instead.".to_string(),
            );
        }
        self.send_revocation(name, "revoked").await?;
        self.refresh_members().await.map(|_| ())
    }

    // ----- views -----

    /// Current, non-content local usage. The weak reference is installed only after the
    /// control center binds its endpoint and expires with that control center.
    pub(crate) fn activity(&self, remote_calls: usize) -> super::node::activity::NodeActivity {
        let runtime = self.runtime_activity.read().upgrade();
        let (connections, requests) = runtime.as_ref().map_or((0, 0), |runtime| runtime.counts());
        super::node::activity::NodeActivity {
            engine_hosted: runtime.is_some(),
            agent_connections: connections,
            local_requests: requests,
            remote_calls,
            observed_at: super::now_rfc3339(),
        }
    }

    /// Updates the signed usage facts when counts change, and at least once per minute.
    /// Hardware collection retains its own timestamp and cadence.
    pub(crate) fn publish_activity(&self, remote_calls: usize) -> Result<(), String> {
        let Some(mut inventory) = self.own_inventory.read().clone() else {
            return Ok(());
        };
        let activity = self.activity(remote_calls);
        if inventory.activity.as_ref().is_some_and(|previous| {
            previous.same_counts(&activity)
                && super::parse_rfc3339(&previous.observed_at).is_ok_and(|observed| {
                    time::OffsetDateTime::now_utc() - observed < time::Duration::seconds(60)
                })
        }) {
            return Ok(());
        }
        inventory.activity = Some(activity);
        let result = self.publish_inventory(&inventory);
        *self.inventory_error.write() = result.as_ref().err().cloned();
        result
    }

    pub(crate) fn nodes(&self) -> Vec<NodeView> {
        self.node_views(false)
    }

    /// The control view includes revoked names, whose identities remain permanently reserved.
    pub(crate) fn all_node_views(&self) -> Vec<NodeView> {
        self.node_views(true)
    }

    fn node_views(&self, include_revoked: bool) -> Vec<NodeView> {
        let me = self.name();
        let members = self.members.read();
        let inventories = self.inventories.read();
        let own = self.own_inventory.read().clone();
        let mut views = members
            .members
            .values()
            .filter(|member| include_revoked || member.is_current())
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
                    private_note: None,
                }
            })
            .collect::<Vec<_>>();
        if include_revoked {
            for (name, revocation) in &members.revoked {
                if views.iter().any(|node| node.name == *name) {
                    continue;
                }
                let inventory = if *name == me {
                    own.clone()
                } else {
                    inventories
                        .get(name)
                        .map(|(_, inventory)| inventory.clone())
                };
                views.push(NodeView {
                    name: name.clone(),
                    state: "revoked".to_string(),
                    last_seen: revocation
                        .parse()
                        .map(|statement| statement.at)
                        .unwrap_or_default(),
                    os: inventory
                        .as_ref()
                        .map(|inventory| inventory.os.clone())
                        .unwrap_or_default(),
                    arch: inventory
                        .as_ref()
                        .map(|inventory| inventory.arch.clone())
                        .unwrap_or_default(),
                    tags: inventory
                        .as_ref()
                        .map(|inventory| inventory.tags.clone())
                        .unwrap_or_default(),
                    version: inventory
                        .as_ref()
                        .map(|inventory| inventory.version.clone())
                        .unwrap_or_default(),
                    hub_rtt_ms: None,
                    tls: None,
                    network: None,
                    inventory,
                    is_self: *name == me,
                    private_note: None,
                });
            }
        }
        views.sort_by(|left, right| left.name.cmp(&right.name));
        views
    }

    pub(crate) fn member(&self, name: &str) -> Option<VerifiedMember> {
        self.members.read().get(name).cloned()
    }

    /// Whether `name` is a member this one may call, with the reason it is not.
    pub(crate) fn resolve_target(&self, name: &str) -> Result<(), String> {
        self.members.read().resolve(name)
    }

    /// Commits an opaque JSON letter locally before returning its globally unique id.
    pub(crate) fn send_mail(&self, to: &str, body: serde_json::Value) -> Result<String, String> {
        let id = hex::encode(super::crypto::random_bytes::<32>()?);
        self.send_mail_as(to, &id, body)?;
        Ok(id)
    }

    /// Commits a letter under an id the sender chose. The same letter sent again under the same
    /// id is delivered once; a different letter under an id already used is refused.
    pub(crate) fn send_mail_as(&self, to: &str, id: &str, body: serde_json::Value) -> Result<(), String> {
        self.resolve_target(to)?;
        self.send_reliable(
            Header::new(kind::MAIL, &self.name(), to, 0),
            &super::mail::Letter {
                id: id.to_string(),
                body,
            },
        )
        .map(|_| ())
    }
    pub(crate) fn inbox(&self) -> Result<Vec<super::mail::InboxItem>, String> {
        let _lock = self.inbox_lock.lock();
        super::mail::list(&self.paths)
    }
    /// Waits until the durable inbox holds letters other than `skipped` (receipts the consumer
    /// already opened but could not acknowledge), and returns those. Registration precedes the
    /// scan to avoid lost wakeups.
    pub(crate) async fn next_mail(
        &self,
        skipped: &std::collections::HashSet<String>,
    ) -> Result<Vec<super::mail::InboxItem>, String> {
        loop {
            let notified = self.inbox_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let inbox: Vec<_> = self
                .inbox()?
                .into_iter()
                .filter(|item| !skipped.contains(&item.receipt))
                .collect();
            if !inbox.is_empty() {
                return Ok(inbox);
            }
            notified.await;
        }
    }

    /// Waits for the inbox to change.
    pub(crate) async fn mail_changed(&self) {
        self.inbox_changed.notified().await;
    }
    /// Commits consumer completion before deleting the inbox file. Idempotent.
    pub(crate) fn acknowledge_mail(&self, receipt: &str) -> Result<(), String> {
        let _lock = self.inbox_lock.lock();
        super::mail::acknowledge(&self.paths, receipt)
    }
    pub(crate) async fn call_many(
        &self,
        targets: Vec<String>,
        verb: &str,
        args: serde_json::Value,
        timeout: Duration,
    ) -> Vec<NodeOutcome> {
        futures_util::future::join_all(targets.iter().map(|target| async {
            self.call(
                verb,
                target,
                args.clone(),
                CallBudget::default(),
                timeout,
                CallOrigin::Script,
            )
            .await
            .unwrap_or_else(|error| NodeOutcome::status(target, "error", error))
        }))
        .await
    }

    pub(crate) async fn hub_request<T: Serialize>(
        &self,
        kind: &str,
        body: &T,
    ) -> Result<Opened, String> {
        let answer = self
            .request(
                Header::new(kind, &self.name(), HUB_NAME, 0),
                body,
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await?
            .single(kind)?;
        expect_kind(&answer, messages::kind::HUB_RESULT)?;
        Ok(answer)
    }
    pub(crate) async fn append_ledger(
        &self,
        scope: String,
        files: Vec<super::ledger::LedgerFile>,
        project: Option<super::ledger::ProjectUpdate>,
    ) -> Result<super::ledger::Row, String> {
        let mut entry = super::ledger::Entry {
            scope,
            id: String::new(),
            author: self.name(),
            files,
            project,
        };
        entry.id = entry.content_id()?;
        entry.validate()?;
        let signed = super::ledger::SignedEntry::sign(&entry, self.identity.signing())?;
        let answer = self.hub_request(kind::LEDGER_APPEND, &signed).await?;
        let row = messages::decode(&answer.body, kind::LEDGER_APPEND)?;
        self.sync_ledger().await?;
        Ok(row)
    }
    pub(crate) async fn sync_ledger(&self) -> Result<(), String> {
        let _lock = self.ledger_sync.lock().await;
        let answer = self.hub_request(kind::LEDGER_TOWNS, &()).await?;
        let towns: BTreeMap<String, super::ledger::Town> =
            messages::decode(&answer.body, kind::LEDGER_TOWNS)?;
        for (scope, town) in towns {
            let mut cursor = super::ledger::cursors(&self.paths)?
                .get(&scope)
                .copied()
                .unwrap_or(0);
            while cursor < town.seq {
                let answer = self
                    .hub_request(kind::LEDGER_NEXT, &(scope.clone(), cursor))
                    .await?;
                let row: super::ledger::Row = messages::decode::<Option<super::ledger::Row>>(
                    &answer.body,
                    kind::LEDGER_NEXT,
                )?
                .ok_or("Hub omitted a ledger entry")?;
                if row.signed.parse()?.scope != scope || row.seq != cursor + 1 {
                    return Err("Hub returned a different ledger position".into());
                }
                super::ledger::apply(&self.paths, &row, &self.members.read())?;
                cursor = row.seq;
            }
        }
        Ok(())
    }
    pub(crate) fn read_ledger(&self, scope: &str) -> Result<super::ledger::View, String> {
        super::ledger::read_local(&self.paths, scope, &self.members.read())
    }

    /// A fresh subscription id for [`Self::subscribe`].
    pub(crate) fn subscription_id() -> Result<String, String> {
        Ok(hex::encode(super::crypto::random_bytes::<32>()?))
    }

    /// Subscribes to `peer` under `id`. The caller names the subscription before asking, so it
    /// can be ready for whatever comes under that id, its end included, before any of it can
    /// arrive.
    pub(crate) async fn subscribe(
        &self,
        peer: &str,
        id: &str,
        topic: serde_json::Value,
    ) -> Result<(), String> {
        self.resolve_target(peer)?;
        {
            let mut active = self.subscriptions.active.lock();
            if active.contains_key(id) {
                return Err("Subscription id already used".into());
            }
            active.insert(
                id.to_string(),
                super::subscription::Active {
                    peer: peer.into(),
                    publishing: false,
                },
            );
        }
        let mut attempt = SubscriptionAttempt {
            client: self,
            id: id.to_string(),
            accepted: false,
        };
        let result = self
            .request(
                Header::new(kind::SUBSCRIBE, &self.name(), peer, 0),
                &super::subscription::Message {
                    id: id.to_string(),
                    value: topic,
                },
                1,
                HUB_REQUEST_TIMEOUT,
            )
            .await;
        result
            .and_then(|answers| answers.single(kind::SUBSCRIBE))
            .and_then(|answer| expect_kind(&answer, kind::HUB_RESULT))?;
        attempt.accepted = true;
        Ok(())
    }
    pub(crate) fn unsubscribe(&self, id: &str) -> Result<(), String> {
        let peer = self
            .subscriptions
            .active
            .lock()
            .get(id)
            .ok_or("Unknown subscription")?
            .peer
            .clone();
        let result = self.send_ephemeral(
            kind::UNSUBSCRIBE,
            &peer,
            &super::subscription::Message {
                id: id.into(),
                value: serde_json::Value::Null,
            },
        );
        self.subscriptions.close(id, "unsubscribed");
        result
    }
    pub(crate) fn publish_event(&self, id: &str, value: serde_json::Value) -> Result<(), String> {
        let active = self
            .subscriptions
            .active
            .lock()
            .get(id)
            .cloned()
            .ok_or("Unknown subscription")?;
        if !active.publishing {
            return Err("This endpoint is the subscriber".into());
        }
        self.send_ephemeral(
            kind::SUBSCRIPTION_EVENT,
            &active.peer,
            &super::subscription::Message {
                id: id.into(),
                value,
            },
        )
    }
    fn send_ephemeral<T: Serialize>(&self, kind: &str, to: &str, body: &T) -> Result<(), String> {
        let env = self.build_envelope(Header::new(kind, &self.name(), to, 0), body)?;
        let frame = Frame::Msg {
            seq: None,
            id: None,
            env,
        };
        frame.encode()?;
        if self.send_frame(frame) {
            Ok(())
        } else {
            Err(self.unreachable_error())
        }
    }
    pub(crate) fn receive_subscription(&self, opened: &Opened) -> Result<(), String> {
        let body: super::subscription::Message = messages::decode(&opened.body, &opened.header.t)?;
        let from = &opened.header.from;
        match opened.header.t.as_str() {
            kind::SUBSCRIBE => {
                if self.subscriptions.active.lock().contains_key(&body.id) {
                    return Err("Subscription id already used".into());
                }
                self.subscriptions.active.lock().insert(
                    body.id.clone(),
                    super::subscription::Active {
                        peer: from.clone(),
                        publishing: true,
                    },
                );
                self.subscriptions
                    .push(super::subscription::Notice::Started {
                        id: body.id,
                        from: from.clone(),
                        topic: body.value,
                    });
            }
            kind::SUBSCRIPTION_CLOSED if from == HUB_NAME => self
                .subscriptions
                .close(&body.id, body.value.as_str().unwrap_or("peer disconnected")),
            kind::UNSUBSCRIBE | kind::SUBSCRIPTION_EVENT => {
                let active = self
                    .subscriptions
                    .active
                    .lock()
                    .get(&body.id)
                    .cloned()
                    .ok_or("Unknown subscription")?;
                if &active.peer != from {
                    return Err("Subscription peer mismatch".into());
                }
                if opened.header.t == kind::UNSUBSCRIBE {
                    self.subscriptions.close(&body.id, "peer unsubscribed");
                } else if !active.publishing {
                    self.subscriptions.push(super::subscription::Notice::Event {
                        id: body.id,
                        from: from.clone(),
                        value: body.value,
                    });
                } else {
                    return Err("Subscriber cannot publish events".into());
                }
            }
            _ => return Err("Invalid subscription message".into()),
        }
        Ok(())
    }

    // ----- direct calls -----

    /// Runs `verb` with `args` on `target` and returns its outcome. A target that is offline,
    /// forbidden, or silent produces a status outcome, not an error. The working directory
    /// of a `run` travels inside `args`; the envelope's own `cwd` is left unset.
    pub(crate) async fn call(
        &self,
        verb: &str,
        target: &str,
        args: serde_json::Value,
        budget: CallBudget,
        tool_timeout: Duration,
        origin: CallOrigin,
    ) -> Result<NodeOutcome, String> {
        if !self.is_connected() {
            return Err(self.unreachable_error());
        }
        let me = self.name();
        let window = tool_timeout + LINK_MARGIN;
        let deadline = tokio::time::Instant::now() + window;
        // The hub judges grants by the verb in the header; the receiving side of a copy is
        // authorized as `copy`, whatever the wire verb says.
        let header = Header::new(kind::CALL, &me, target, 0)
            .with_verb(super::node::executor::grant_verb(verb))
            .with_wait(window.as_secs());
        let body = Call {
            verb: verb.to_string(),
            args,
            budget,
            cwd: None,
            timeout_ms: tool_timeout.as_millis() as u64,
            grant_revision: self.grant_revision(),
            origin,
        };
        let Answers {
            answers,
            delivered,
            link_lost,
        } = self.request(header, &body, 1, window).await?;
        for answer in answers {
            match answer.header.t.as_str() {
                kind::CALL_RESULT | kind::CALL_RESULT_STREAM => {
                    if answer.header.from != target {
                        return Err("Answer author differs from called member".into());
                    }
                    let result: CallResult = if answer.header.t == kind::CALL_RESULT_STREAM {
                        let description = messages::decode(&answer.body, kind::CALL_RESULT_STREAM)?;
                        tokio::time::timeout_at(
                            deadline,
                            super::result_stream::receive(self, description, target),
                        )
                        .await
                        .map_err(|_| "Call result stream exceeded caller deadline")??
                    } else {
                        messages::decode(&answer.body, kind::CALL_RESULT)?
                    };
                    // A result answers for the member that sent it, and only for the one this
                    // call was addressed to; both fields sit inside the signed body.
                    if result.node != answer.header.from || result.node != target {
                        continue;
                    }
                    self.raise_grant_floor(result.grant_revision)?;
                    return Ok(NodeOutcome {
                        node: result.node,
                        status: if result.response.is_error {
                            "error"
                        } else {
                            "ok"
                        }
                        .to_string(),
                        response: Some(result.response.into()),
                        message: None,
                        facts: result.facts,
                    });
                }
                kind::CALL_STATUS if answer.header.from == HUB_NAME => {
                    let Ok(status) =
                        messages::decode::<CallStatus>(&answer.body, kind::CALL_STATUS)
                    else {
                        continue;
                    };
                    if status.node != target {
                        continue;
                    }
                    return Ok(NodeOutcome::status(
                        &status.node,
                        &status.status,
                        status.message,
                    ));
                }
                _ => {}
            }
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
        Ok(NodeOutcome::status(target, status, message))
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
