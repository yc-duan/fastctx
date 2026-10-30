//! Keeping the World's tools in this machine's logins as its fastctx writes
//! them (`world-agent-logins.md`, 接入跟着版本走): each login they are in is
//! brought up to date by itself when the node starts, after a scan and when
//! one of the files the World writes in it changes (`watch`), one change at
//! a time with the user's own; what could not be done stays in the record
//! until a change of that login settles it. While the World program is being
//! replaced nothing is counted against a login: it is tried again shortly,
//! and once the replacement is done.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_install::apply::{self, Error, Host, Refresh};
use fastctx_types::AgentName;
use fastctx_types::logins::Drift;

use super::Context;
use crate::Node;

/// Each login waiting for the user, and what it waits for.
type Waiting = Vec<(AgentName, PathBuf, Drift)>;

/// How soon a login is tried again when the World program was being
/// replaced.
const AGAIN: Duration = Duration::from_secs(30);

/// Brings every login the World's tools are in up to date.
pub(crate) async fn all(node: &Arc<Node>) {
    let _changing = node.logins.changing.lock().await;
    let refreshing = node.clone();
    match tokio::task::spawn_blocking(move || every(&refreshing)).await {
        Ok(Some((waiting, again))) => {
            *node.logins.drift.lock().expect("never poisoned") = waiting;
            if again {
                later(Arc::downgrade(node));
            }
        }
        Ok(None) => {}
        Err(error) => node.log.line(format_args!("bringing the World's tools up to date: {error}")),
    }
}

/// The World program's replacement is done: the logins that waited for it
/// are brought up to date now, not at the next hourly look
/// (`world-agent-logins.md`, 换完了接着就补上).
pub(crate) async fn replaced(node: &Arc<Node>) {
    if node.logins.after_replacement.swap(false, Ordering::AcqRel) {
        all(node).await;
    }
}

/// Tries again once the World program is likely in place.
fn later(node: Weak<Node>) {
    tokio::spawn(async move {
        tokio::time::sleep(AGAIN).await;
        if let Some(node) = node.upgrade() {
            all(&node).await;
        }
    });
}

/// Whether a replacement of the World program is under way: what places it
/// waits for that, and is no fault of the login's.
fn replacing(node: &Node) -> bool {
    use fastctx_types::renewal::Phase;
    fastctx_install::renewal::read(&node.root)
        .ok()
        .flatten()
        .is_some_and(|plan| !matches!(plan.phase, Phase::Complete | Phase::Failed))
}

/// Whether the last replacement of the World program failed: the machine's
/// update says so, and what could not be placed meanwhile is no fault of the
/// login's either. It is tried again on its own schedule; once it is done,
/// `replaced` brings the logins up to date.
fn replacement_failed(node: &Node) -> bool {
    fastctx_install::renewal::read(&node.root)
        .ok()
        .flatten()
        .is_some_and(|plan| plan.phase == fastctx_types::renewal::Phase::Failed)
}

/// The logins left waiting for the user, and whether to try again shortly;
/// none when they could not be read.
fn every(node: &Node) -> Option<(Waiting, bool)> {
    let context = match Context::of(node) {
        Ok(context) => context,
        Err(failure) => {
            node.log.line(format_args!("bringing the World's tools up to date: {}", failure.message));
            return None;
        }
    };
    let integrations = match apply::integrations(&context.places) {
        Ok(integrations) => integrations,
        Err(error) => {
            node.log.line(format_args!("bringing the World's tools up to date: reading the apply record: {error}"));
            return None;
        }
    };
    let mut waiting = Vec::new();
    let mut again = false;
    for integration in integrations {
        let agent = AgentName::try_from(match integration.host {
            Host::Codex => "codex",
            Host::Claude => "claude",
        })
        .expect("an agent name");
        let dir = fastctx_hosts::inventory::plain(&integration.home);
        let refreshed = context
            .places
            .at_home(integration.host, &integration.home)
            .and_then(|places| apply::refresh(&places, integration.host));
        let shown = dir.display();
        match refreshed {
            Ok(Refresh::Out | Refresh::Current) => {}
            Ok(Refresh::Refreshed { files, commands, kept }) => {
                let done =
                    files.iter().map(|file| file.display().to_string()).chain(commands).collect::<Vec<_>>().join(", ");
                let kept =
                    kept.map(|kept| format!("; what was taken back is kept in {}", kept.display())).unwrap_or_default();
                node.log.line(format_args!(
                    "brought the World's tools in the {agent} login {shown} up to date: {done}{kept}"
                ));
            }
            Ok(Refresh::Changed { location, reason }) => {
                node.log.line(format_args!(
                    "could not bring the World's tools in the {agent} login {shown} up to date: {location}: {reason}"
                ));
                waiting.push((agent, dir, Drift::Changed { location, reason }));
            }
            // Said by the renewal holding its gates, or while its plan says
            // it is under way: whatever the plan read a moment later says.
            Err(error) if matches!(error, Error::Replacing(_)) || replacing(node) => {
                node.log.line(format_args!(
                    "the World program is being replaced: the World's tools in the {agent} login {shown} are brought up to date once it is ({error})"
                ));
                if let Some(drift) = of(node, agent.as_str(), &dir) {
                    waiting.push((agent, dir, drift));
                }
                again = true;
            }
            Err(error) if replacement_failed(node) => {
                node.log.line(format_args!(
                    "the World program could not be replaced: the World's tools in the {agent} login {shown} are brought up to date once it is ({error})"
                ));
                if let Some(drift) = of(node, agent.as_str(), &dir) {
                    waiting.push((agent, dir, drift));
                }
                node.logins.after_replacement.store(true, Ordering::Release);
            }
            Err(error) => {
                node.log.line(format_args!(
                    "could not bring the World's tools in the {agent} login {shown} up to date: {error}"
                ));
                let location = shown.to_string();
                waiting.push((agent, dir, Drift::Changed { location, reason: error.to_string() }));
            }
        }
    }
    Some((waiting, again))
}

/// What the login `dir` of `agent` waits for, if anything.
pub(crate) fn of(node: &Node, agent: &str, dir: &Path) -> Option<Drift> {
    let waiting = node.logins.drift.lock().expect("never poisoned");
    waiting
        .iter()
        .find(|(owner, path, _)| owner.as_str() == agent && crate::homes::same_dir(path, dir))
        .map(|(_, _, drift)| drift.clone())
}

/// A change of the login settled what it waited for.
pub(crate) fn settled(node: &Node, agent: &AgentName, dir: &Path) {
    node.logins
        .drift
        .lock()
        .expect("never poisoned")
        .retain(|(owner, path, _)| !(owner == agent && crate::homes::same_dir(path, dir)));
}
