//! Doctor checks for the World: enrollment files, the node service, the hub link and its
//! network path, the login shell, and a hub running on this machine.

use super::client::LinkState;
use super::node::admin::NodeStatus;
use super::{WorldPaths, is_enrolled, load_config};
use crate::control::doctor::DoctorCheck;
use crate::control::paths::ControlPaths;

/// A status file older than this belongs to a daemon that is no longer writing.
const STATUS_FRESH_SECONDS: i64 = 20;

/// The World section of `fastctx status`.
pub(crate) fn checks(paths: &ControlPaths) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let world_paths = WorldPaths::from_control(paths);
    if !is_enrolled(paths) {
        checks.push(DoctorCheck::info(
            "World enrollment",
            "This machine is not enrolled in a World; the tool surface is the local one.",
        ));
        checks.extend(hub_check(paths));
        return checks;
    }
    let config = match load_config(&world_paths) {
        Ok(Some(config)) => config,
        Ok(None) => {
            checks.push(DoctorCheck::fail(
                "World enrollment",
                "world.toml vanished while being checked.",
                "Run fastctx node status.",
            ));
            return checks;
        }
        Err(error) => {
            checks.push(DoctorCheck::fail(
                "World enrollment",
                error,
                "Repair ~/.fastctx/world.toml, or run fastctx node unenroll and enroll again.",
            ));
            return checks;
        }
    };
    let status = match super::node::read_status(&world_paths) {
        Ok(status) => status,
        Err(error) => {
            checks.push(DoctorCheck::fail(
                "World node status",
                error,
                "Repair the reported status file or restart the node.",
            ));
            None
        }
    };
    let live = status.as_ref().is_some_and(status_is_fresh);
    let mut missing = Vec::new();
    for path in [&world_paths.identity_key] {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
            Ok(_) => missing.push(format!(
                "{} is not a nonempty key file",
                crate::paths::display_path(path)
            )),
            Err(error) => missing.push(format!(
                "Cannot inspect {}: {error}",
                crate::paths::display_path(path)
            )),
        }
    }
    if !missing.is_empty() {
        checks.push(DoctorCheck::fail(
            "World enrollment",
            missing.join("; "),
            "Repair the enrollment files, or leave and join with a fresh invite.",
        ));
    } else if let Some(status) = status.as_ref().filter(|_| live) {
        checks.push(DoctorCheck::pass(
            "World enrollment",
            format!(
                "Enrolled as \"{}\" in World {} via {} (node-reported member key {}).",
                config.name,
                config.world_id,
                config.hub.join(", "),
                status.fingerprint
            ),
        ));
    } else {
        checks.push(DoctorCheck::info("World enrollment",format!("Enrolled as \"{}\" in World {} via {}. Key files are present; start the node to validate them. Control interfaces never load private keys.",config.name,config.world_id,config.hub.join(", "))));
    }
    let installed = super::node::service::inspect(paths);
    checks.push(match (installed, live) {
        (Ok(true), true) => DoctorCheck::pass(
            "World node service",
            format!(
                "Installed and running (pid {}).",
                status.as_ref().map_or(0, |status| status.pid)
            ),
        ),
        (Ok(true), false) => DoctorCheck::fail(
            "World node service",
            "The service is installed but no node is writing status; it is not running.",
            "Run fastctx node restart, then fastctx node status.",
        ),
        (Ok(false), true) => DoctorCheck::info(
            "World node service",
            format!(
                "A node is running (pid {}) without a service registration; it will not survive a logout.",
                status.as_ref().map_or(0, |status| status.pid)
            ),
        ),
        (Ok(false), false) => DoctorCheck::fail(
            "World node service",
            "No service is installed and no node is running; World calls fail with node_service_not_running.",
            "Run fastctx node install-service.",
        ),
        (Err(error),_) => DoctorCheck::fail("World node service",error,"Inspect the OS service manager using the same account and HOME as the node."),
    });

    let Some(status) = status.filter(|_| live) else {
        checks.extend(hub_check(paths));
        return checks;
    };
    checks.extend(link_checks(&status));
    checks.extend(hub_check(paths));
    checks
}

pub(crate) fn status_is_fresh(status: &NodeStatus) -> bool {
    super::parse_rfc3339(&status.written_at)
        .map(|written| {
            (time::OffsetDateTime::now_utc() - written).whole_seconds() < STATUS_FRESH_SECONDS
        })
        .unwrap_or(false)
}

fn seconds_since(text: &Option<String>) -> Option<i64> {
    let text = text.as_deref()?;
    let moment = super::parse_rfc3339(text).ok()?;
    Some((time::OffsetDateTime::now_utc() - moment).whole_seconds())
}

/// Why a stopped link will not retry on its own, and therefore what the user must do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopRemedy {
    /// This member was removed from the World; only a fresh invite brings it back.
    Revoked,
    /// The two sides speak incompatible protocol versions.
    ProtocolMismatch,
    /// The hub's identity or TLS mode changed since enrollment.
    HubIdentityMismatch,
    /// Anything else: restart the node once the cause is fixed.
    Restart,
}

/// One structured observation about this node's hub link.
///
/// Doctor renders these as English text; the terminal control surface localises them. Keeping
/// the rules here means the two never disagree about what counts as healthy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkFinding {
    Connected {
        since: Option<String>,
    },
    Connecting {
        attempt: u32,
        last_error: Option<String>,
    },
    Reconnecting {
        attempt: u32,
        next_attempt_at: String,
        last_error: Option<String>,
    },
    Stopped {
        reason: String,
        until: Option<String>,
        remedy: StopRemedy,
    },
    /// A TUN adapter is active and the link is pinned to a physical interface that bypasses it.
    TunBypassed {
        tunnels: Vec<String>,
        interface: String,
    },
    NetworkPath(String),
    /// Acknowledgements are late or the outbox is backing up; the hub is not keeping up.
    AckLate {
        ack_age: Option<i64>,
        outbox: usize,
    },
    /// Another process presented this member's key; presence will keep flapping.
    Replaced(u32),
    HealthOk {
        heartbeat_age: Option<i64>,
        ack_age: Option<i64>,
    },
    /// A newer grant revision exists than this node holds; remote calls are refused meanwhile.
    GrantStale,
    /// The node runs but does not host the control center, so World calls cannot reach it.
    NotHosting,
    Hosting,
}

pub(crate) fn link_findings(status: &NodeStatus) -> Vec<LinkFinding> {
    let link = &status.link;
    let mut findings = vec![match &link.state {
        LinkState::Connected => LinkFinding::Connected {
            since: link.connected_since.clone(),
        },
        LinkState::Starting => LinkFinding::Connecting {
            attempt: 0,
            last_error: link.last_error.clone(),
        },
        LinkState::Connecting { attempt } => LinkFinding::Connecting {
            attempt: *attempt,
            last_error: link.last_error.clone(),
        },
        LinkState::Reconnecting {
            attempt,
            next_attempt_at,
        } => LinkFinding::Reconnecting {
            attempt: *attempt,
            next_attempt_at: next_attempt_at.clone(),
            last_error: link.last_error.clone(),
        },
        LinkState::Stopped { reason, until } => LinkFinding::Stopped {
            reason: reason.clone(),
            until: until.clone(),
            remedy: if reason.starts_with("revoked") {
                StopRemedy::Revoked
            } else if reason.starts_with("protocol_mismatch") {
                StopRemedy::ProtocolMismatch
            } else if reason.starts_with("hub_identity_mismatch") {
                StopRemedy::HubIdentityMismatch
            } else {
                StopRemedy::Restart
            },
        },
    }];
    if let (false, Some(interface)) = (link.tunnels.is_empty(), link.interface.as_deref()) {
        findings.push(LinkFinding::TunBypassed {
            tunnels: link.tunnels.clone(),
            interface: interface.to_string(),
        });
    } else if let Some(path) = &link.path {
        findings.push(LinkFinding::NetworkPath(path.clone()));
    }
    if link.is_connected() {
        let heartbeat_age = seconds_since(&link.last_heartbeat_at);
        let ack_age = seconds_since(&link.last_ack_at);
        findings.push(
            if ack_age.is_some_and(|age| age > 30) || link.outbox_depth > 100 {
                LinkFinding::AckLate {
                    ack_age,
                    outbox: link.outbox_depth,
                }
            } else if link.replaced_count > 0 {
                LinkFinding::Replaced(link.replaced_count)
            } else {
                LinkFinding::HealthOk {
                    heartbeat_age,
                    ack_age,
                }
            },
        );
    }
    if status.grant_stale {
        findings.push(LinkFinding::GrantStale);
    }
    findings.push(if status.engine_hosted {
        LinkFinding::Hosting
    } else {
        LinkFinding::NotHosting
    });
    findings
}

pub(crate) fn link_checks(status: &NodeStatus) -> Vec<DoctorCheck> {
    let link = &status.link;
    let hubs = status.hub.join(", ");
    let last_error = |error: &Option<String>| {
        error
            .as_deref()
            .map(|error| format!("; last error: {error}"))
            .unwrap_or_default()
    };
    let health_detail = |heartbeat_age: Option<i64>, ack_age: Option<i64>| {
        format!(
            "Heartbeat {} ago, ack {} ago, rtt {}, outbox {}, replaced {} time(s).",
            heartbeat_age.map_or("never".to_string(), |age| format!("{age} s")),
            ack_age.map_or("never".to_string(), |age| format!("{age} s")),
            link.rtt_ms
                .map_or("?".to_string(), |rtt| format!("{rtt} ms")),
            link.outbox_depth,
            link.replaced_count
        )
    };
    let members_detail = format!(
        "{} known, {} online; grant revision {}; {} running remote call(s).",
        status.members, status.members_online, status.grant_revision, status.running_calls
    );
    let mut members_reported = false;
    let mut checks = Vec::new();
    for finding in link_findings(status) {
        let check = match finding {
            LinkFinding::Connected { since } => DoctorCheck::pass(
                "World hub link",
                format!(
                    "Connected to {hubs} since {}; network {}; TLS {}.",
                    since.as_deref().unwrap_or("?"),
                    link.path.as_deref().unwrap_or("?"),
                    link.tls.as_str()
                ),
            ),
            LinkFinding::Connecting {
                last_error: error, ..
            } => DoctorCheck::info(
                "World hub link",
                format!("Connecting to {hubs}{}.", last_error(&error)),
            ),
            LinkFinding::Reconnecting {
                attempt,
                next_attempt_at,
                last_error: error,
            } => DoctorCheck::fail(
                "World hub link",
                format!(
                    "Reconnecting to {hubs} (attempt {attempt}, next at {next_attempt_at}){}.",
                    last_error(&error)
                ),
                "Check that the hub is running and reachable from this network; fastctx node status shows every attempt.",
            ),
            LinkFinding::Stopped {
                reason,
                until,
                remedy,
            } => DoctorCheck::fail(
                "World hub link",
                match until {
                    Some(until) => format!("Paused until {until}: {reason}"),
                    None => format!("Stopped: {reason}"),
                },
                match remedy {
                    StopRemedy::Revoked => {
                        "This member was removed from the World; run fastctx node unenroll and ask for a new invite."
                    }
                    StopRemedy::ProtocolMismatch => {
                        "Upgrade fastctx on the older side, then fastctx node restart."
                    }
                    StopRemedy::HubIdentityMismatch => {
                        "The hub's identity or TLS mode changed; unenroll and enroll again with a fresh invite."
                    }
                    StopRemedy::Restart => "Run fastctx node restart once the cause is fixed.",
                },
            ),
            LinkFinding::TunBypassed { tunnels, interface } => DoctorCheck::info(
                "World network path",
                format!(
                    "A TUN adapter is active ({}); the hub link is pinned to {interface} and bypasses it.",
                    tunnels.join(", ")
                ),
            ),
            LinkFinding::NetworkPath(path) => DoctorCheck::info("World network path", path),
            LinkFinding::AckLate { ack_age, .. } => DoctorCheck::fail(
                "World link health",
                health_detail(seconds_since(&link.last_heartbeat_at), ack_age),
                "The hub is not acknowledging in time; fastctx node status shows the reconnect state.",
            ),
            LinkFinding::Replaced(_) => DoctorCheck::fail(
                "World link health",
                health_detail(
                    seconds_since(&link.last_heartbeat_at),
                    seconds_since(&link.last_ack_at),
                ),
                "Another process used this member's key; make sure only one fastctx node runs for this user.",
            ),
            LinkFinding::HealthOk {
                heartbeat_age,
                ack_age,
            } => DoctorCheck::pass("World link health", health_detail(heartbeat_age, ack_age)),
            LinkFinding::GrantStale => {
                members_reported = true;
                DoctorCheck::fail(
                    "World members",
                    members_detail.clone(),
                    "A newer grant revision exists than this node holds; calls from other members are refused as grant_stale until the hub delivers it.",
                )
            }
            LinkFinding::Hosting | LinkFinding::NotHosting => {
                if !members_reported {
                    members_reported = true;
                    checks.push(DoctorCheck::info("World members", members_detail.clone()));
                }
                if finding == LinkFinding::Hosting {
                    DoctorCheck::pass(
                        "World control center",
                        "The node service hosts this machine's FastCtx control center.",
                    )
                } else {
                    DoctorCheck::fail(
                        "World control center",
                        "The node service is running but not hosting the control center; World calls will fail with node_service_not_running.",
                        "Run fastctx node restart; if it persists, check the node log for the endpoint takeover error.",
                    )
                }
            }
        };
        checks.push(check);
    }
    checks
}

/// A hub running from the default data directory on this machine.
fn hub_check(paths: &ControlPaths) -> Option<DoctorCheck> {
    let data = paths.home.join(".fastctx-hub");
    let status = super::hub::read_status(&data).ok().flatten()?;
    let live = super::hub::status_is_live(&status);
    let online = status
        .members
        .iter()
        .filter(|member| member.state == "online")
        .count();
    Some(if live {
        DoctorCheck::pass(
            "World hub",
            format!(
                "Running on {} (World {}, {} members, {online} online, TLS: {}).",
                status.listen,
                status.world_id,
                status.members.len(),
                status.tls
            ),
        )
    } else {
        DoctorCheck::info(
            "World hub",
            format!(
                "A hub was running from {} (last status {}); it is not running now.",
                crate::paths::display_path(&data),
                status.written_at
            ),
        )
    })
}
