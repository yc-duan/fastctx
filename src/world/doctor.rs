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
    match (
        super::identity::Identity::load(&world_paths),
        super::keys::KeyRing::load(&world_paths),
    ) {
        (Ok(Some(identity)), Ok(Some(keys))) => checks.push(DoctorCheck::pass(
            "World enrollment",
            format!(
                "Enrolled as \"{}\" in World {} via {} (member key {}, World key epoch {}).",
                config.name,
                config.world_id,
                config.hub.join(", "),
                identity.fingerprint(),
                keys.current().epoch()
            ),
        )),
        (Ok(None), _) | (_, Ok(None)) => checks.push(DoctorCheck::fail(
            "World enrollment",
            "world.toml exists but the identity or World key files are missing.",
            "Run fastctx node unenroll, then enroll again with a fresh invite.",
        )),
        (Err(error), _) | (_, Err(error)) => checks.push(DoctorCheck::fail(
            "World enrollment",
            error,
            "Run fastctx node unenroll, then enroll again with a fresh invite.",
        )),
    }

    let installed = super::node::service::is_installed();
    let status = super::node::read_status(&world_paths).ok().flatten();
    let live = status.as_ref().is_some_and(status_is_fresh);
    checks.push(match (installed, live) {
        (true, true) => DoctorCheck::pass(
            "World node service",
            format!(
                "Installed and running (pid {}).",
                status.as_ref().map_or(0, |status| status.pid)
            ),
        ),
        (true, false) => DoctorCheck::fail(
            "World node service",
            "The service is installed but no node is writing status; it is not running.",
            "Run fastctx node restart, then fastctx node status.",
        ),
        (false, true) => DoctorCheck::info(
            "World node service",
            format!(
                "A node is running (pid {}) without a service registration; it will not survive a logout.",
                status.as_ref().map_or(0, |status| status.pid)
            ),
        ),
        (false, false) => DoctorCheck::fail(
            "World node service",
            "No service is installed and no node is running; World calls fail with node_service_not_running.",
            "Run fastctx node install-service.",
        ),
    });

    let Some(status) = status.filter(|_| live) else {
        checks.extend(hub_check(paths));
        return checks;
    };
    checks.extend(link_checks(&status));
    checks.extend(hub_check(paths));
    checks
}

fn status_is_fresh(status: &NodeStatus) -> bool {
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

fn link_checks(status: &NodeStatus) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let link = &status.link;
    checks.push(match &link.state {
        LinkState::Connected => DoctorCheck::pass(
            "World hub link",
            format!(
                "Connected to {} since {}; network {}; TLS {}.",
                status.hub.join(", "),
                link.connected_since.as_deref().unwrap_or("?"),
                link.path.as_deref().unwrap_or("?"),
                link.tls.as_str()
            ),
        ),
        LinkState::Starting | LinkState::Connecting { .. } => DoctorCheck::info(
            "World hub link",
            format!(
                "Connecting to {}{}.",
                status.hub.join(", "),
                link.last_error
                    .as_deref()
                    .map(|error| format!("; last error: {error}"))
                    .unwrap_or_default()
            ),
        ),
        LinkState::Reconnecting {
            attempt,
            next_attempt_at,
        } => DoctorCheck::fail(
            "World hub link",
            format!(
                "Reconnecting to {} (attempt {attempt}, next at {next_attempt_at}){}.",
                status.hub.join(", "),
                link.last_error
                    .as_deref()
                    .map(|error| format!("; last error: {error}"))
                    .unwrap_or_default()
            ),
            "Check that the hub is running and reachable from this network; fastctx node status shows every attempt.",
        ),
        LinkState::Stopped { reason, until } => DoctorCheck::fail(
            "World hub link",
            match until {
                Some(until) => format!("Paused until {until}: {reason}"),
                None => format!("Stopped: {reason}"),
            },
            if reason.starts_with("revoked") {
                "This member was removed from the World; run fastctx node unenroll and ask for a new invite."
            } else if reason.starts_with("protocol_mismatch") {
                "Upgrade fastctx on the older side, then fastctx node restart."
            } else if reason.starts_with("hub_identity_mismatch") {
                "The hub's identity or TLS mode changed; unenroll and enroll again with a fresh invite."
            } else {
                "Run fastctx node restart once the cause is fixed."
            },
        ),
    });
    if !link.tunnels.is_empty() {
        checks.push(DoctorCheck::info(
            "World network path",
            format!(
                "A TUN adapter is active ({}); the hub link is pinned to {} and bypasses it.",
                link.tunnels.join(", "),
                link.interface
                    .as_deref()
                    .unwrap_or("the physical interface")
            ),
        ));
    } else if let Some(path) = &link.path {
        checks.push(DoctorCheck::info("World network path", path.clone()));
    }
    if link.is_connected() {
        let heartbeat_age = seconds_since(&link.last_heartbeat_at);
        let ack_age = seconds_since(&link.last_ack_at);
        let detail = format!(
            "Heartbeat {} ago, ack {} ago, rtt {}, outbox {}, key epoch {}, replaced {} time(s).",
            heartbeat_age.map_or("never".to_string(), |age| format!("{age} s")),
            ack_age.map_or("never".to_string(), |age| format!("{age} s")),
            link.rtt_ms
                .map_or("?".to_string(), |rtt| format!("{rtt} ms")),
            link.outbox_depth,
            link.key_epoch,
            link.replaced_count
        );
        checks.push(if ack_age.is_some_and(|age| age > 30) || link.outbox_depth > 100 {
            DoctorCheck::fail(
                "World link health",
                detail,
                "The hub is not acknowledging in time; fastctx node status shows the reconnect state.",
            )
        } else if link.replaced_count > 0 {
            DoctorCheck::fail(
                "World link health",
                detail,
                "Another process used this member's key; make sure only one fastctx node runs for this user.",
            )
        } else {
            DoctorCheck::pass("World link health", detail)
        });
    }
    checks.push(DoctorCheck::info(
        "World members",
        format!(
            "{} known, {} online; grants version {}; {} running remote call(s).",
            status.members, status.members_online, status.grant_version, status.running_calls
        ),
    ));
    checks.push(if status.engine_hosted {
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
    });
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
