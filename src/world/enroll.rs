//! Joining a World: `fastctx world init` for the first machine and `fastctx node enroll` for
//! every later one. Both dial the hub once with a learning TLS verifier, authenticate the
//! hub against the invite (or the bootstrap password), obtain the World keys, and write the
//! enrollment files. The daemon takes over from there.

use super::client::Inventory;
use super::crypto::{b64_array, b64_decode, b64_encode};
use super::identity::{Fingerprint, Identity, verify};
use super::invite::Invite;
use super::keys::KeyRing;
use super::link::{self, DialPlan, Dialed, Endpoint, Learned, Verify};
use super::messages::{self, kind};
use super::wire::{self, Auth, BindingMode, Enrollment, Frame, Hello, Intent, Welcome};
use super::{NetworkMode, PROTOCOL_VERSION, TlsMode, WorldConfig, WorldPaths};
use crate::control::paths::ControlPaths;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Options shared by both enrollment paths.
#[derive(Clone, Debug)]
pub(crate) struct EnrollOptions {
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) network: NetworkMode,
    pub(crate) interface: Option<String>,
}

/// What the caller prints after a successful enrollment.
#[derive(Clone, Debug)]
pub(crate) struct EnrollSummary {
    pub(crate) name: String,
    pub(crate) world_id: String,
    pub(crate) hub: Vec<String>,
    pub(crate) hub_key: Fingerprint,
    pub(crate) tls: TlsMode,
    pub(crate) path: String,
    pub(crate) key_epoch: u32,
}

enum Admission {
    Bootstrap { password: String },
    Invite(Invite),
}

/// The first machine: creates the World key and admits itself with the bootstrap password.
pub(crate) async fn bootstrap(
    paths: &ControlPaths,
    hub: &str,
    password: &str,
    options: EnrollOptions,
) -> Result<EnrollSummary, String> {
    let endpoint = Endpoint::parse(hub)?;
    enroll_with(
        paths,
        vec![endpoint],
        None,
        Admission::Bootstrap {
            password: password.to_string(),
        },
        options,
    )
    .await
}

/// Every later machine: presents an invite and receives the World keys wrapped under it.
pub(crate) async fn enroll(
    paths: &ControlPaths,
    invite_text: &str,
    mut options: EnrollOptions,
) -> Result<EnrollSummary, String> {
    let invite = Invite::parse(invite_text)?;
    if invite.is_expired_at(time::OffsetDateTime::now_utc()) {
        return Err(format!(
            "That invite expired at {}. Ask a member for a new one.",
            invite.exp
        ));
    }
    if options.name.is_empty() {
        options.name = invite.name.clone().ok_or_else(|| {
            "The invite carries no suggested name; pass --name <machine-name>.".to_string()
        })?;
    }
    let endpoints = invite
        .hub
        .iter()
        .map(|text| Endpoint::parse(text))
        .collect::<Result<Vec<_>, _>>()?;
    let hub_key = invite.hub_key;
    enroll_with(
        paths,
        endpoints,
        Some(hub_key),
        Admission::Invite(invite),
        options,
    )
    .await
}

async fn enroll_with(
    paths: &ControlPaths,
    endpoints: Vec<Endpoint>,
    expected_hub_key: Option<Fingerprint>,
    admission: Admission,
    options: EnrollOptions,
) -> Result<EnrollSummary, String> {
    super::validate_node_name(&options.name)?;
    for tag in &options.tags {
        super::validate_node_name(tag).map_err(|_| {
            format!("Invalid tag \"{tag}\": use lowercase letters, digits, and hyphens.")
        })?;
    }
    let world_paths = WorldPaths::from_control(paths);
    if super::is_enrolled(paths) {
        return Err(format!(
            "This machine is already enrolled ({} exists). Run 'fastctx node unenroll' first.",
            crate::paths::display_path(&world_paths.config)
        ));
    }
    world_paths.ensure()?;
    let identity = Identity::generate()?;
    let learned = Arc::new(Mutex::new(None::<Learned>));
    let plan = DialPlan {
        endpoints: endpoints.clone(),
        mode: options.network,
        interface: options.interface.clone(),
        preferred: None,
    };
    let mut dialed = link::dial(&plan, &Verify::Learn(Arc::clone(&learned)))
        .await
        .map_err(|failure| format!("Cannot reach the hub: {failure}"))?;
    let outcome = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        handshake(
            &mut dialed,
            &identity,
            expected_hub_key,
            &admission,
            &options,
        ),
    )
    .await
    .map_err(|_| "The hub did not finish the enrollment handshake within 10 s.".to_string())?;
    let (welcome, hub_key) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            dialed.close().await;
            return Err(error);
        }
    };
    let learned = learned
        .lock()
        .clone()
        .ok_or_else(|| "The TLS handshake recorded no certificate.".to_string())?;
    let tls = decide_tls(&learned, &welcome_binding_mode(&dialed))?;
    let keys = match (
        &admission,
        welcome
            .enrolled
            .as_ref()
            .and_then(|enrolled| enrolled.wrapped_keys.as_ref()),
    ) {
        (Admission::Bootstrap { .. }, _) => KeyRing::new_initial()?,
        (Admission::Invite(invite), Some(wrapped)) => invite.unwrap_keys(wrapped)?,
        (Admission::Invite(_), None) => {
            dialed.close().await;
            return Err("The hub admitted this machine but sent no World keys; the invite may have been created by a hub without members.".to_string());
        }
    };
    let config = WorldConfig {
        version: 1,
        name: options.name.clone(),
        world_id: welcome.world_id.clone(),
        hub: endpoints.iter().map(ToString::to_string).collect(),
        hub_key: hub_key.to_string(),
        tls,
        pinned_spki_sha256: (tls == TlsMode::Pinned).then(|| learned.spki_sha256.clone()),
        network: options.network,
        interface: options.interface.clone(),
        enrolled_at: super::now_rfc3339(),
    };
    identity.save(&world_paths)?;
    keys.save(&world_paths)?;
    let mut state = super::state::NodeState::load(&world_paths)?;
    state.last_network = Some(dialed.path.mode());
    state.save(&world_paths)?;
    super::save_config(&world_paths, &config)?;

    // Publish the record and inventory on this same connection so the hub lists the new
    // member immediately, then let the daemon own the link.
    let client = super::client::WorldClient::open(world_paths.clone())?
        .ok_or_else(|| "The enrollment files were written but cannot be read back.".to_string())?;
    let inventory: Inventory = super::node::inventory::collect(&client).await;
    let record_env = {
        let record = messages::MemberRecord {
            name: config.name.clone(),
            node_pub: b64_encode(&identity.public_key()),
            wrap_pub: b64_encode(&identity.wrap_public()),
            tags: options.tags.clone(),
            kind: "stateful".to_string(),
            os: inventory.os.clone(),
            arch: inventory.arch.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            enrolled_at: config.enrolled_at.clone(),
        };
        let signed = super::members::publish_record(&identity, &keys, &record)?;
        let header =
            super::envelope::Header::new(kind::MEMBER_PUBLISH, &config.name, super::HUB_NAME, 0);
        client.send_reliable(header, &messages::MemberPublish { signed }, false, true)?;
        let header =
            super::envelope::Header::new(kind::INVENTORY, &config.name, super::HUB_NAME, 0);
        client.send_reliable(header, &inventory, true, false)?;
        client.outbox_after(0)?
    };
    let mut expected_acks = Vec::new();
    for (seq, entry) in record_env {
        dialed.send(&Frame::reliable(seq, entry.env)).await?;
        expected_acks.push(seq);
    }
    let ack_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !expected_acks.is_empty() {
        match tokio::time::timeout_at(ack_deadline, dialed.recv()).await {
            Ok(Ok(Some(Frame::Ack { seq }))) => {
                expected_acks.retain(|pending| *pending > seq);
                client.outbox_ack(seq)?;
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        }
    }
    let _ = dialed
        .send(&Frame::Bye {
            reason: "enrollment complete".to_string(),
        })
        .await;
    dialed.close().await;
    Ok(EnrollSummary {
        name: config.name,
        world_id: config.world_id,
        hub: config.hub,
        hub_key,
        tls,
        path: dialed.path.describe(),
        key_epoch: keys.current().epoch(),
    })
}

fn welcome_binding_mode(dialed: &Dialed) -> BindingMode {
    if dialed.binding.is_empty() {
        BindingMode::None
    } else {
        BindingMode::Exporter
    }
}

/// Which TLS mode to record. A hub that disclaims the channel binding is only accepted
/// behind a publicly trusted certificate: anything else could be a man in the middle.
fn decide_tls(learned: &Learned, binding: &BindingMode) -> Result<TlsMode, String> {
    match (binding, learned.webpki_ok) {
        (BindingMode::None, true) => Ok(TlsMode::Fronted),
        (BindingMode::None, false) => Err(format!(
            "hub_identity_mismatch: the hub disclaims the TLS channel binding (it says a proxy fronts it) but its certificate is not publicly trusted ({}). Refusing to enroll.",
            learned.webpki_error.clone().unwrap_or_default()
        )),
        (BindingMode::Exporter, true) => Ok(TlsMode::Webpki),
        (BindingMode::Exporter, false) => Ok(TlsMode::Pinned),
    }
}

async fn handshake(
    dialed: &mut Dialed,
    identity: &Identity,
    expected_hub_key: Option<Fingerprint>,
    admission: &Admission,
    options: &EnrollOptions,
) -> Result<(Welcome, Fingerprint), String> {
    let node_nonce = super::crypto::random_bytes::<32>()?;
    let intent = match admission {
        Admission::Bootstrap { .. } => Intent::Bootstrap,
        Admission::Invite(_) => Intent::Enroll,
    };
    dialed
        .send(&Frame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            min_protocol: PROTOCOL_VERSION.saturating_sub(1).max(1),
            version: env!("CARGO_PKG_VERSION").to_string(),
            nonce: b64_encode(&node_nonce),
            node_pub: b64_encode(&identity.public_key()),
            wrap_pub: b64_encode(&identity.wrap_public()),
            intent,
        }))
        .await?;
    let challenge = match dialed.recv().await? {
        Some(Frame::Challenge(challenge)) => challenge,
        Some(Frame::Rejected(rejected)) => {
            return Err(format!("{}: {}", rejected.code, rejected.message));
        }
        Some(_) => {
            return Err(
                "The hub answered hello with something other than a challenge.".to_string(),
            );
        }
        None => return Err("The hub closed the connection during the handshake.".to_string()),
    };
    let hub_pub = b64_array::<32>(&challenge.hub_pub, "hub public key")?;
    let hub_key = Fingerprint::of(&hub_pub);
    if let Some(expected) = expected_hub_key
        && hub_key != expected
    {
        return Err(format!(
            "hub_identity_mismatch: the hub at {} presented key {hub_key}, but the invite names {expected}. Refusing to enroll.",
            dialed.endpoint
        ));
    }
    let binding = match challenge.binding {
        BindingMode::Exporter => dialed.binding.clone(),
        BindingMode::None => {
            dialed.binding.clear();
            Vec::new()
        }
    };
    let hub_nonce = b64_decode(&challenge.nonce)?;
    let node_pub = identity.public_key();
    let transcript = wire::hub_transcript(&node_nonce, &hub_nonce, &hub_pub, &node_pub, &binding);
    let signature = b64_decode(&challenge.sig)?;
    verify(&hub_pub, wire::HUB_HANDSHAKE_DOMAIN, &transcript, &signature).map_err(|_| {
        "hub_identity_mismatch: the hub's challenge signature does not verify; the connection may be intercepted. Refusing to enroll.".to_string()
    })?;
    let node_transcript =
        wire::node_transcript(&hub_nonce, &node_nonce, &node_pub, &hub_pub, &binding);
    let (code_id, admission_token) = match admission {
        Admission::Bootstrap { password } => (
            "bootstrap".to_string(),
            super::crypto::hmac_sha256(password.as_bytes(), b"admission"),
        ),
        Admission::Invite(invite) => (invite.code_id(), invite.admission_token()),
    };
    dialed
        .send(&Frame::Auth(Auth {
            sig: b64_encode(&identity.sign(wire::NODE_HANDSHAKE_DOMAIN, &node_transcript)),
            recv_seq: 0,
            enrollment: Some(Enrollment {
                code_id,
                admission_token: b64_encode(&admission_token),
                name: options.name.clone(),
                tags: options.tags.clone(),
            }),
        }))
        .await?;
    match dialed.recv().await? {
        Some(Frame::Welcome(welcome)) => Ok((welcome, hub_key)),
        Some(Frame::Rejected(rejected)) => Err(format!("{}: {}", rejected.code, rejected.message)),
        Some(_) => Err("The hub answered auth with something other than welcome.".to_string()),
        None => Err("The hub closed the connection after auth.".to_string()),
    }
}
