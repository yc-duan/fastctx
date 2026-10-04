//! Opening a resident in its host's own interface (`fastctx attach`): the
//! user, at the machine the resident lives on, takes up the same session in
//! the official program, beside the World's own view of it
//! (`world-gui.md`, 它是什么).

use std::sync::Arc;

use fastctx_hosts::claude::background;
use fastctx_types::ResidentId;
use fastctx_types::calls::HostCommand;
use fastctx_types::ipc::Failure;

use crate::Node;

/// The command that opens `resident` in its host's own interface.
pub async fn command(node: &Arc<Node>, resident: &ResidentId) -> Result<HostCommand, Failure> {
    let here = node.machine();
    let (about, _) = node.residents.about(resident).ok_or_else(|| {
        Failure::new(format!(
            "no resident {resident} lives on {here}; a session is opened in its own interface on the machine it lives on"
        ))
    })?;
    if !about.world_started {
        return Err(Failure::new(format!(
            "{resident} is a session you opened yourself, in {}; it is open there already",
            about.host.client
        )));
    }
    let session =
        about.session.ok_or_else(|| Failure::new(format!("{resident}'s session has not started on its host yet")))?;
    match about.host.agent.as_str() {
        // A background session is attached to by the short id `claude --bg`
        // printed, which is how Claude Code itself names it to the user.
        "claude" => {
            let path = std::env::var_os("PATH");
            let program = background::find(path.as_ref())
                .ok_or_else(|| Failure::new(format!("Claude Code is not installed on {here}: no claude command")))?;
            let short = session.get(..8).unwrap_or(&session).to_string();
            Ok(HostCommand { program, args: vec!["attach".to_string(), short], env: Vec::new() })
        }
        // The official Codex joins the World's own app-server on its socket,
        // with the thread loaded under the World's settings first.
        "codex" => {
            let (program, address, codex_home) = node.codex.attachable(&session).await.map_err(Failure::new)?;
            Ok(HostCommand {
                program,
                args: vec!["--remote".to_string(), address, "resume".to_string(), session],
                env: vec![("CODEX_HOME".to_string(), codex_home.to_string_lossy().into_owned())],
            })
        }
        other => {
            Err(Failure::new(format!("{resident} lives in {other}, which the World cannot open in its own interface")))
        }
    }
}
