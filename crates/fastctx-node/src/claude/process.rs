//! Observes the process lifetime registered by the Claude mod.

use crate::Node;
use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;
use std::sync::Arc;

pub(super) fn observe(node: &Arc<Node>, process: Option<fastctx_types::ProcessIdentity>) {
    let Some(process) = process else { return };
    if !node.claude.state.lock().expect("never poisoned").observed.insert(process.clone()) {
        match fastctx_platform::is_alive(&process) {
            Ok(false) => {}
            Ok(true) => return,
            Err(error) => {
                node.log.line(format_args!("checking the Claude host process: {error}"));
                return;
            }
        }
    }
    let node = node.clone();
    tokio::spawn(async move {
        let identity = process.clone();
        match tokio::task::spawn_blocking(move || fastctx_platform::host_process::wait_for_exit(&identity)).await {
            Ok(Ok(())) => {}
            error => {
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
            )
            .await
            {
                node.log.line(format_args!("settling the dead Claude session {session}: {error}"));
            }
        }
    });
}
