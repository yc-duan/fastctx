//! Observes the process lifetime registered by the Claude mod.

use crate::Node;
use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;
use std::sync::Arc;

fn unobserve(node: &Node, process: &fastctx_types::ProcessIdentity) {
    let mut state = node.claude.state.lock().expect("never poisoned");
    state.observed.remove(process);
    if state.observed.is_empty() {
        state.observed = Default::default();
    }
}

/// An observation error is not an exit. Keep observing, independently of
/// the notification subscription, until the exact process identity ends.
pub(super) async fn wait(node: &Node, process: &fastctx_types::ProcessIdentity) {
    let mut problem = None;
    loop {
        let result = fastctx_platform::host_process::exited(process).await;
        if result.is_ok() || fastctx_platform::has_ended(process).is_ok_and(|ended| ended) {
            return;
        }
        let error = result.expect_err("an unsuccessful observation").to_string();
        if problem.as_ref() != Some(&error) {
            node.log.line(format_args!("watching a Claude host process; retrying: {error}"));
            problem = Some(error);
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

pub(super) fn observe(node: &Arc<Node>, process: Option<fastctx_types::ProcessIdentity>) {
    let Some(process) = process else { return };
    if !node.claude.state.lock().expect("never poisoned").observed.insert(process.clone()) {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let exited = wait(&node, &process);
        tokio::pin!(exited);
        loop {
            let changed = node.claude.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !node
                .claude
                .state
                .lock()
                .expect("never poisoned")
                .sessions
                .values()
                .any(|session| session.registration.process.as_ref() == Some(&process))
            {
                unobserve(&node, &process);
                return;
            }
            tokio::select! {
                () = &mut exited => break,
                () = changed => {},
            }
        }
        let sessions: Vec<(String, ResidentId)> = {
            let state = node.claude.state.lock().expect("never poisoned");
            state
                .sessions
                .iter()
                .filter(|(_, entry)| entry.registration.process.as_ref() == Some(&process))
                .filter_map(|(session, entry)| Some((session.clone(), entry.resident.clone()?)))
                .collect()
        };
        for (session, resident) in sessions {
            let directory = exchange::sessions(node.world.path()).join(&session);
            let mut problem = None;
            loop {
                // A final flushed just before death wins over inferred interruption.
                super::look(&node, &directory).await;
                match super::ended_for_process(
                    &node,
                    &resident,
                    &session,
                    Some("its host process exited".to_string()),
                    Some(&process),
                    None,
                )
                .await
                {
                    Ok(()) => break,
                    Err(error) => {
                        let error = error.to_string();
                        if problem.as_ref() != Some(&error) {
                            node.log
                                .line(format_args!("settling the dead Claude session {session}; retrying: {error}"));
                            problem = Some(error);
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
        unobserve(&node, &process);
    });
}
