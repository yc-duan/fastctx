//! A Claude session whose Claude Code runs on with the World's plugin gone
//! from it. Claude Code unloads a plugin it judges runaway, or one whose
//! hooks worker crashed, and loads it in that process no more (2.1.303):
//! from then on no tool call of the session is paired, nothing reaches it
//! from its inbox and no turn of it is reported, while the session itself
//! still looks present. The plugin's control helper ends with it
//! (`claude_request::guard`). Once it has, and a while later no other
//! activation followed and this one was not closed, the session is stopped,
//! for whatever is for it next to start it again, plugin and all, and the
//! people who look at it are told.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fastctx_hosts::claude::control::{self as files, Host};
use fastctx_hosts::claude::exchange;
use fastctx_types::session::{Health, HealthKind};

use crate::Node;

/// How long an ended helper is given to be followed: a plugin updated or
/// reloaded ends its helper before the next one names itself.
const FOLLOWED_WITHIN: Duration = Duration::from_secs(60);

/// Where a session the plugin was gone from tells Claude Code's own account
/// of itself the next time it is woken, and what says it is to: so that, if
/// it goes again, why is on record, which the session's record never says.
/// Claude Code turns the file over at ten megabytes; only such a session
/// writes one.
pub(super) const DEBUG_LOG: &str = "claude-debug.log";
const DEBUG_NEXT: &str = "debug-next";

/// Whether the session in `session_directory` is to be woken telling its
/// account of itself to `DEBUG_LOG`; asked once, as it is woken.
pub(super) fn debugged(session_directory: &Path) -> bool {
    let marker = session_directory.join(DEBUG_NEXT);
    fastctx_platform::disk::is_there(&marker) && fastctx_platform::disk::remove_file(&marker).is_ok()
}

/// What the people who look at the session are told; the interface knows it
/// by its beginning.
pub(crate) const UNLOADED: &str =
    "Claude Code unloaded the World's plugin from this session; the session is started again with it";

/// Looks into an activation whose helper ended, once: the looks at the
/// session that follow find it ended again.
pub(super) async fn check(node: &Arc<Node>, host: &Host, session_directory: &Path) {
    if !gone(node, host, session_directory).unwrap_or(false) {
        return;
    }
    let key = (host.session.clone(), host.instance.clone());
    if !node.claude.bridge.gone.lock().expect("never poisoned").insert(key.clone()) {
        return;
    }
    tokio::time::sleep(FOLLOWED_WITHIN).await;
    let restarted = match gone(node, host, session_directory) {
        Ok(true) => restart(node, host, session_directory).await,
        // Followed or closed after all: a later end is looked into again.
        Ok(false) => false,
        Err(error) => {
            node.log.line(format_args!("looking into Claude session {}'s ended plugin: {error}", host.session));
            false
        }
    };
    if !restarted {
        node.claude.bridge.gone.lock().expect("never poisoned").remove(&key);
    }
}

/// Whether the plugin is gone from the session: the activation is still the
/// one it names, it was not closed, its helper ended, the session's Claude
/// Code runs on, and fastctx is not being removed, which ends the helpers by
/// design.
fn gone(node: &Node, host: &Host, session_directory: &Path) -> std::io::Result<bool> {
    if node.world.path().parent().is_some_and(|root| fastctx_platform::disk::is_there(root.join("uninstalled.json"))) {
        return Ok(false);
    }
    let control = files::directory(node.world.path(), &host.session);
    Ok(files::host(&control)?.as_ref() == Some(host)
        && !fastctx_platform::disk::is_there(control.join("closed").join(format!("{}.json", host.instance)))
        && !fastctx_platform::is_alive(&host.process)?
        && super::control::session_present(node, session_directory)?)
}

/// Stops the session the plugin is gone from, and says so; whether it did.
async fn restart(node: &Arc<Node>, host: &Host, session_directory: &Path) -> bool {
    let session = host.session.as_str();
    let Some(resident) = node.residents.resident_of_thread(session) else { return false };
    let Some((about, _)) = node.residents.about(&resident) else { return false };
    // Only the session a conversation the World holds is now.
    if !about.world_managed() || about.session.as_deref() != Some(session) {
        return false;
    }
    node.log.line(format_args!(
        "the World's plugin is gone from Claude session {session} of {resident}, whose Claude Code runs on; it is stopped, to start again with the plugin"
    ));
    if let Err(error) = fastctx_platform::disk::write(session_directory.join(DEBUG_NEXT), b"") {
        node.log.line(format_args!("asking that {session} be woken with Claude Code's account kept: {error}"));
    }
    let turn = node.residents.now(&resident).and_then(|now| now.turn);
    let health = Health { kind: HealthKind::ToolsMissing, text: UNLOADED.to_string() };
    if let Err(error) = crate::turns::health(node, &resident, turn, health) {
        node.log.line(format_args!("telling {resident} its plugin is gone: {error}"));
    }
    match super::management::stop(node, about.home.as_deref(), &resident, session, false).await {
        Ok(()) => true,
        // A session Claude Code's supervisor does not keep is ended as the
        // process it registered, told apart from a reused process ID.
        Err(failure) => match exchange::registration(session_directory).ok().flatten().and_then(|it| it.process) {
            Some(process) => match fastctx_platform::terminate::terminate(&process) {
                Ok(()) => true,
                Err(error) => {
                    node.log.line(format_args!(
                        "stopping Claude session {session}, its plugin gone: {}; ending its process: {error}",
                        failure.message
                    ));
                    false
                }
            },
            None => {
                node.log.line(format_args!(
                    "stopping Claude session {session}, its plugin gone: {}; it registered no process",
                    failure.message
                ));
                false
            }
        },
    }
}
