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

pub(super) fn observe(node: &Arc<Node>, process: Option<fastctx_types::ProcessIdentity>) {
    let Some(process) = process else { return };
    if !node.claude.state.lock().expect("never poisoned").observed.insert(process.clone()) {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let exited = fastctx_platform::host_process::exited(&process);
        tokio::pin!(exited);
        let result = loop {
            let changed = node.claude.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = node.claude.state.lock().expect("never poisoned");
                if !state.sessions.values().any(|session| session.registration.process.as_ref() == Some(&process)) {
                    // A new activation must see either this observer or an absent entry.
                    state.observed.remove(&process);
                    if state.observed.is_empty() {
                        state.observed = Default::default();
                    }
                    return;
                }
            }
            tokio::select! {
                result = &mut exited => break result,
                () = changed => {},
            }
        };
        match result {
            Ok(()) => {}
            error => {
                unobserve(&node, &process);
                node.log.line(format_args!("watching a Claude host process: {error:?}"));
                return;
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
            // A final flushed just before death wins over inferred interruption.
            super::look(&node, &directory).await;
            if let Err(error) = super::ended_for_process(
                &node,
                &resident,
                &session,
                Some("its host process exited".to_string()),
                Some(&process),
                None,
            )
            .await
            {
                node.log.line(format_args!("settling the dead Claude session {session}: {error}"));
            }
        }
        unobserve(&node, &process);
    });
}
