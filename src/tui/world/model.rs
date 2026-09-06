//! Facts derived for people: presence, verdicts, service stages and localised link findings.
//!
//! Every page reads the same node snapshot; this module turns it into the handful of shapes the
//! views draw so that "is this machine fine" is decided once, with one symbol vocabulary, and
//! not re-derived per page. Symbols carry the meaning and colour only reinforces it, because the
//! sixteen-colour palette collapses muted onto the foreground and monochrome collapses everything.

use super::i18n::{WorldMessages, fill};
use crate::tui::theme;
use crate::world::client::{LinkState, NodeView};
use crate::world::control::{FailureKind, ReadState, Snapshot};
use crate::world::doctor::{LinkFinding, StopRemedy, status_is_fresh};
use crate::world::node::admin::NodeStatus;
use ratatui::style::Color;

/// One of four judgement levels, each with its own symbol so state survives without colour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tone {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Tone {
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Ok => "√",
            Self::Info => "·",
            Self::Warn => "!",
            Self::Fail => "×",
        }
    }
    pub(super) fn color(self) -> Color {
        match self {
            Self::Ok => theme::success(),
            Self::Info => theme::muted(),
            Self::Warn => theme::warning(),
            Self::Fail => theme::danger(),
        }
    }
}

/// A member's presence as the diagram and lists show it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Presence {
    Online,
    Offline,
    Revoked,
    Unknown,
}

impl Presence {
    pub(super) fn from_state(state: &str) -> Self {
        match state {
            "online" => Self::Online,
            "offline" => Self::Offline,
            "revoked" => Self::Revoked,
            _ => Self::Unknown,
        }
    }
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Online => "●",
            Self::Offline => "○",
            Self::Revoked => "×",
            Self::Unknown => "?",
        }
    }
    pub(super) fn label(self, m: &WorldMessages) -> &'static str {
        match self {
            Self::Online => m.online,
            Self::Offline => m.offline,
            Self::Revoked => m.revoked,
            Self::Unknown => m.unknown,
        }
    }
    pub(super) fn color(self) -> Color {
        match self {
            Self::Online => theme::success(),
            Self::Offline => theme::muted(),
            Self::Revoked => theme::danger(),
            Self::Unknown => theme::warning(),
        }
    }
}

/// How this machine's own link reads, reduced to what the diagram needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkTone {
    Connected,
    Connecting,
    Stopped,
}

impl LinkTone {
    pub(super) fn from_state(state: &LinkState) -> Self {
        match state {
            LinkState::Connected => Self::Connected,
            LinkState::Starting | LinkState::Connecting { .. } | LinkState::Reconnecting { .. } => {
                Self::Connecting
            }
            LinkState::Stopped { .. } => Self::Stopped,
        }
    }
    pub(super) fn label(self, m: &WorldMessages) -> &'static str {
        match self {
            Self::Connected => m.connected,
            Self::Connecting => m.connecting,
            Self::Stopped => m.stopped,
        }
    }
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Connected => "●",
            Self::Connecting => "○",
            Self::Stopped => "×",
        }
    }
    pub(super) fn color(self) -> Color {
        match self {
            Self::Connected => theme::success(),
            Self::Connecting => theme::warning(),
            Self::Stopped => theme::danger(),
        }
    }
}

pub(super) fn os_label(os: &str) -> &str {
    match os {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" | "darwin" => "macOS",
        other => other,
    }
}

/// "3 min ago" for an RFC 3339 instant; unparsable or far-future input reads as unknown.
pub(super) fn relative(instant: &str, m: &WorldMessages) -> String {
    let Ok(moment) = crate::world::parse_rfc3339(instant) else {
        return m.unknown.to_string();
    };
    let seconds = (time::OffsetDateTime::now_utc() - moment).whole_seconds();
    if seconds < -5 {
        return m.unknown.to_string();
    }
    let seconds = seconds.max(0);
    let (template, n) = if seconds < 10 {
        return m.just_now.to_string();
    } else if seconds < 60 {
        (m.ago_seconds, seconds)
    } else if seconds < 3600 {
        (m.ago_minutes, seconds / 60)
    } else if seconds < 86_400 {
        (m.ago_hours, seconds / 3600)
    } else {
        (m.ago_days, seconds / 86_400)
    };
    fill(template, &[("n", &n.to_string())])
}

/// Activity a member reported about itself, only while it is fresh enough to be current.
pub(super) fn activity_line(node: &NodeView, m: &WorldMessages) -> Option<String> {
    let activity = node.inventory.as_ref()?.activity.as_ref()?;
    if Presence::from_state(&node.state) != Presence::Online || !activity.is_fresh() {
        return None;
    }
    let calls = activity.local_requests + activity.remote_calls;
    Some(if activity.agent_connections == 0 && calls == 0 {
        m.idle.to_string()
    } else {
        fill(
            m.activity_line,
            &[
                ("sessions", &activity.agent_connections.to_string()),
                ("calls", &calls.to_string()),
            ],
        )
    })
}

/// The most current status this snapshot holds, live or last known.
pub(super) fn current_status(snapshot: &Snapshot) -> Option<(&NodeStatus, bool)> {
    if let Some(node) = snapshot.node.get() {
        return Some((&node.status, status_is_fresh(&node.status)));
    }
    snapshot.last_status.get().map(|status| (status, false))
}

/// What the Overview band says, chosen by severity: the first thing that stops the World from
/// working on this machine wins, and only when nothing does is the member count shown.
pub(super) struct Verdict {
    pub(super) tone: Tone,
    pub(super) text: String,
}

pub(super) fn verdict(snapshot: &Snapshot, m: &WorldMessages) -> Verdict {
    match &snapshot.node {
        ReadState::Missing => {
            return Verdict {
                tone: Tone::Fail,
                text: m.verdict_node_down.to_string(),
            };
        }
        ReadState::Failed(error) => {
            return Verdict {
                tone: Tone::Fail,
                text: if error.kind == FailureKind::Permission {
                    m.permission
                } else {
                    m.verdict_node_error
                }
                .to_string(),
            };
        }
        ReadState::Ready(_) => {}
    }
    let Some((status, fresh)) = current_status(snapshot) else {
        return Verdict {
            tone: Tone::Warn,
            text: m.unavailable.to_string(),
        };
    };
    if !fresh {
        return Verdict {
            tone: Tone::Warn,
            text: m.verdict_stale.to_string(),
        };
    }
    match &status.link.state {
        LinkState::Stopped { .. } => {
            return Verdict {
                tone: Tone::Fail,
                text: fill(m.verdict_link_stopped, &[("reason", m.stopped)]),
            };
        }
        LinkState::Reconnecting { attempt, .. } => {
            return Verdict {
                tone: Tone::Warn,
                text: fill(
                    m.verdict_link_reconnecting,
                    &[("attempt", &attempt.to_string())],
                ),
            };
        }
        LinkState::Starting | LinkState::Connecting { .. } => {
            let attempt = match &status.link.state {
                LinkState::Connecting { attempt } => *attempt,
                _ => 0,
            };
            return Verdict {
                tone: Tone::Warn,
                text: fill(
                    m.verdict_link_connecting,
                    &[("attempt", &attempt.to_string())],
                ),
            };
        }
        LinkState::Connected => {}
    }
    if !status.engine_hosted {
        return Verdict {
            tone: Tone::Fail,
            text: m.verdict_not_hosting.to_string(),
        };
    }
    if status.grant_stale {
        return Verdict {
            tone: Tone::Warn,
            text: m.verdict_grant_stale.to_string(),
        };
    }
    let unverified = snapshot.node.get().map_or(0, |node| node.unverified.len());
    if unverified > 0 {
        return Verdict {
            tone: Tone::Warn,
            text: fill(m.verdict_unverified, &[("count", &unverified.to_string())]),
        };
    }
    let members = fill(
        m.members_online,
        &[
            ("online", &status.members_online.to_string()),
            ("total", &status.members.to_string()),
        ],
    );
    if status.members_online < status.members {
        let offline = status.members - status.members_online;
        return Verdict {
            tone: Tone::Info,
            text: format!(
                "{} · {}",
                fill(m.verdict_offline, &[("count", &offline.to_string())]),
                members
            ),
        };
    }
    Verdict {
        tone: Tone::Ok,
        text: format!("{} · {}", m.verdict_ok, members),
    }
}

/// Where this machine stands, as a chain of dependent steps; the first non-green step is the
/// one to fix, which is also what decides the recommended service action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Enrolled,
    Service,
    Process,
    Link,
    Remote,
}

impl Stage {
    pub(super) fn label(self, m: &WorldMessages) -> &'static str {
        match self {
            Self::Enrolled => m.stage_enrolled,
            Self::Service => m.stage_service,
            Self::Process => m.stage_process,
            Self::Link => m.stage_link,
            Self::Remote => m.stage_remote,
        }
    }
}

pub(super) struct StageView {
    pub(super) stage: Stage,
    pub(super) tone: Tone,
    pub(super) detail: String,
}

/// The service facts every page agrees on.
pub(super) struct ServiceFacts {
    pub(super) installed: Option<bool>,
    pub(super) running: Option<bool>,
    pub(super) link: Option<LinkTone>,
    pub(super) hosting: Option<bool>,
}

pub(super) fn service_facts(snapshot: &Snapshot) -> ServiceFacts {
    let running = match &snapshot.node {
        ReadState::Ready(_) => Some(true),
        ReadState::Missing => Some(false),
        ReadState::Failed(_) => None,
    };
    let live = snapshot
        .node
        .get()
        .map(|node| &node.status)
        .filter(|status| status_is_fresh(status));
    ServiceFacts {
        installed: snapshot.service_installed.get().copied(),
        running,
        link: live.map(|status| LinkTone::from_state(&status.link.state)),
        hosting: live.map(|status| status.engine_hosted),
    }
}

pub(super) fn stages(snapshot: &Snapshot, m: &WorldMessages) -> Vec<StageView> {
    let facts = service_facts(snapshot);
    let (status, fresh) = current_status(snapshot)
        .map(|(status, fresh)| (Some(status), fresh))
        .unwrap_or((None, false));
    let mut views = Vec::new();
    views.push(StageView {
        stage: Stage::Enrolled,
        tone: Tone::Ok,
        detail: snapshot
            .config
            .get()
            .map(|config| config.name.clone())
            .unwrap_or_else(|| m.unknown.to_string()),
    });
    views.push(match (&snapshot.service_installed, facts.installed) {
        (ReadState::Failed(error), _) => StageView {
            stage: Stage::Service,
            tone: Tone::Warn,
            detail: if error.kind == FailureKind::Permission {
                m.permission
            } else {
                m.error
            }
            .to_string(),
        },
        (_, Some(true)) => StageView {
            stage: Stage::Service,
            tone: Tone::Ok,
            detail: m.installed.to_string(),
        },
        (_, Some(false)) => StageView {
            stage: Stage::Service,
            tone: Tone::Warn,
            detail: m.not_installed.to_string(),
        },
        (_, None) => StageView {
            stage: Stage::Service,
            tone: Tone::Info,
            detail: m.unknown.to_string(),
        },
    });
    views.push(match (&snapshot.node, facts.running) {
        (ReadState::Failed(error), _) => StageView {
            stage: Stage::Process,
            tone: Tone::Fail,
            detail: if error.kind == FailureKind::Permission {
                m.permission
            } else {
                m.verdict_node_error
            }
            .to_string(),
        },
        (_, Some(true)) if fresh => StageView {
            stage: Stage::Process,
            tone: Tone::Ok,
            detail: status
                .map(|_| m.running.to_string())
                .unwrap_or_else(|| m.running.to_string()),
        },
        (_, Some(true)) => StageView {
            stage: Stage::Process,
            tone: Tone::Warn,
            detail: m.stale.to_string(),
        },
        _ => StageView {
            stage: Stage::Process,
            tone: Tone::Fail,
            detail: m.not_running.to_string(),
        },
    });
    views.push(match facts.link {
        Some(tone) => StageView {
            stage: Stage::Link,
            tone: match tone {
                LinkTone::Connected => Tone::Ok,
                LinkTone::Connecting => Tone::Warn,
                LinkTone::Stopped => Tone::Fail,
            },
            detail: match status.map(|status| &status.link.state) {
                Some(LinkState::Stopped { .. }) => m.stopped.to_string(),
                Some(LinkState::Reconnecting { attempt, .. })
                | Some(LinkState::Connecting { attempt }) => {
                    format!("{} · {attempt}", m.connecting)
                }
                _ => tone.label(m).to_string(),
            },
        },
        None => StageView {
            stage: Stage::Link,
            tone: Tone::Info,
            detail: m.unknown.to_string(),
        },
    });
    views.push(match facts.hosting {
        Some(true) => StageView {
            stage: Stage::Remote,
            tone: Tone::Ok,
            detail: m.yes.to_string(),
        },
        Some(false) => StageView {
            stage: Stage::Remote,
            tone: Tone::Fail,
            detail: m.no.to_string(),
        },
        None => StageView {
            stage: Stage::Remote,
            tone: Tone::Info,
            detail: m.unknown.to_string(),
        },
    });
    views
}

/// A localised link finding: judgement, sentence, and the step that fixes it when there is one.
pub(super) struct FindingView {
    pub(super) tone: Tone,
    pub(super) text: String,
    pub(super) remedy: Option<&'static str>,
}

pub(super) fn finding_view(finding: &LinkFinding, m: &WorldMessages) -> FindingView {
    match finding {
        LinkFinding::Connected { since } => FindingView {
            tone: Tone::Ok,
            text: fill(
                m.find_connected,
                &[(
                    "since",
                    &since
                        .as_deref()
                        .map(|since| relative(since, m))
                        .unwrap_or_else(|| m.unknown.to_string()),
                )],
            ),
            remedy: None,
        },
        LinkFinding::Connecting {
            attempt,
            last_error,
        } => FindingView {
            tone: Tone::Warn,
            text: with_error(
                fill(m.find_connecting, &[("attempt", &attempt.to_string())]),
                last_error,
            ),
            remedy: Some(m.remedy_reconnect),
        },
        LinkFinding::Reconnecting {
            attempt,
            next_attempt_at,
            last_error,
        } => FindingView {
            tone: Tone::Warn,
            text: with_error(
                fill(
                    m.find_reconnecting,
                    &[("attempt", &attempt.to_string()), ("at", next_attempt_at)],
                ),
                last_error,
            ),
            remedy: Some(m.remedy_reconnect),
        },
        LinkFinding::Stopped {
            reason,
            until,
            remedy,
        } => FindingView {
            tone: Tone::Fail,
            text: match until {
                Some(until) => fill(m.find_paused, &[("until", until), ("reason", reason)]),
                None => fill(m.find_stopped, &[("reason", reason)]),
            },
            remedy: Some(match remedy {
                StopRemedy::Revoked => m.remedy_revoked,
                StopRemedy::ProtocolMismatch => m.remedy_protocol,
                StopRemedy::HubIdentityMismatch => m.remedy_identity,
                StopRemedy::Restart => m.remedy_restart,
            }),
        },
        LinkFinding::TunBypassed { tunnels, interface } => FindingView {
            tone: Tone::Info,
            text: fill(
                m.find_tun,
                &[("tunnels", &tunnels.join(", ")), ("interface", interface)],
            ),
            remedy: None,
        },
        LinkFinding::NetworkPath(path) => FindingView {
            tone: Tone::Info,
            text: fill(m.find_path, &[("path", path)]),
            remedy: None,
        },
        LinkFinding::AckLate { ack_age, outbox } => FindingView {
            tone: Tone::Fail,
            text: fill(
                m.find_ack_late,
                &[
                    (
                        "ago",
                        &ack_age
                            .map(|age| fill(m.ago_seconds, &[("n", &age.to_string())]))
                            .unwrap_or_else(|| m.none.to_string()),
                    ),
                    ("outbox", &outbox.to_string()),
                ],
            ),
            remedy: Some(m.remedy_ack),
        },
        LinkFinding::Replaced(count) => FindingView {
            tone: Tone::Fail,
            text: fill(m.find_replaced, &[("count", &count.to_string())]),
            remedy: Some(m.remedy_replaced),
        },
        LinkFinding::HealthOk { .. } => FindingView {
            tone: Tone::Ok,
            text: m.find_health_ok.to_string(),
            remedy: None,
        },
        LinkFinding::GrantStale => FindingView {
            tone: Tone::Warn,
            text: m.find_grant_stale.to_string(),
            remedy: Some(m.remedy_grant_stale),
        },
        LinkFinding::NotHosting => FindingView {
            tone: Tone::Fail,
            text: m.find_not_hosting.to_string(),
            remedy: Some(m.remedy_not_hosting),
        },
        LinkFinding::Hosting => FindingView {
            tone: Tone::Ok,
            text: m.find_hosting.to_string(),
            remedy: None,
        },
    }
}

fn with_error(text: String, error: &Option<String>) -> String {
    match error {
        Some(error) => format!("{text} · {error}"),
        None => text,
    }
}

pub(super) fn event_label<'a>(kind: &'a str, m: &'a WorldMessages) -> (&'a str, Tone) {
    match kind {
        "node.online" => (m.ev_node_online, Tone::Ok),
        "node.offline" => (m.ev_node_offline, Tone::Info),
        "node.enrolled" => (m.ev_node_enrolled, Tone::Ok),
        "node.revoked" => (m.ev_node_revoked, Tone::Fail),
        "invite.created" => (m.ev_invite_created, Tone::Info),
        "key.rotated" => (m.ev_key_rotated, Tone::Warn),
        "grant.changed" => (m.ev_grant_changed, Tone::Warn),
        other => (other, Tone::Info),
    }
}
