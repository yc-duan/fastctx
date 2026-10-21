//! Keeping the World's tools in this machine's logins as its fastctx writes
//! them (`world-agent-logins.md`, 接入跟着版本走): each login they are in is
//! brought up to date by itself when the node starts, after a scan and when
//! one of the files the World writes in it changes (`watch`), one change at
//! a time with the user's own; what could not be done stays in the record
//! until a change of that login settles it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_install::apply::{self, Host, Refresh};
use fastctx_types::AgentName;
use fastctx_types::logins::Drift;

use super::Context;
use crate::Node;

/// Brings every login the World's tools are in up to date.
pub(crate) async fn all(node: &Arc<Node>) {
    let _changing = node.logins.changing.lock().await;
    let refreshing = node.clone();
    match tokio::task::spawn_blocking(move || every(&refreshing)).await {
        Ok(Some(waiting)) => *node.logins.drift.lock().expect("never poisoned") = waiting,
        Ok(None) => {}
        Err(error) => node.log.line(format_args!("bringing the World's tools up to date: {error}")),
    }
}

/// The logins left waiting for the user, or none when they could not be
/// read.
fn every(node: &Node) -> Option<Vec<(AgentName, PathBuf, Drift)>> {
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
                let done = files.iter().map(|file| file.display().to_string()).chain(commands).collect::<Vec<_>>().join(", ");
                let kept = kept.map(|kept| format!("; what was taken back is kept in {}", kept.display())).unwrap_or_default();
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
            Err(error) => {
                node.log.line(format_args!(
                    "could not bring the World's tools in the {agent} login {shown} up to date: {error}"
                ));
                let location = shown.to_string();
                waiting.push((agent, dir, Drift::Changed { location, reason: error.to_string() }));
            }
        }
    }
    Some(waiting)
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
