//! `copy`: one file or one directory tree from one machine to another, rsync style
//! (`design-protocol.md` §7). The call goes to the machine holding the source, which asks the
//! receiving machine to open a data connection and then streams a manifest and the wanted
//! files through the hub; the receiving machine answers which files it already has, writes
//! the rest beside their final names, and renames each into place once its hash checks out.
//!
//! One call, one source, one destination, one object. Several machines are several calls.

pub(crate) mod sink;
pub(crate) mod source;
mod walk;

use crate::session::SessionContext;
use crate::world::client::WorldClient;
use crate::world::messages::Call;
use crate::world::node::executor::VerbOutcome;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// What is being copied.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ObjectKind {
    File,
    Dir,
}

/// The arguments of a `copy` call as they reach the machine holding the source.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CopyArgs {
    pub(crate) from: String,
    pub(crate) to: String,
    /// The machine receiving the copy; always explicit on the wire.
    pub(crate) to_node: String,
    /// The machine that asked. The head note leaves its name out of the endpoints, so the
    /// caller reads its own paths bare and the other machine's with a prefix.
    pub(crate) caller: String,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
}

/// `copy_receive`: what the source tells the destination before the stream opens.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReceiveArgs {
    pub(crate) to: String,
    pub(crate) kind: ObjectKind,
    /// The machine that will attach as the stream's source.
    pub(crate) from_node: String,
    /// Milliseconds the destination keeps its end open before giving up.
    pub(crate) deadline_ms: u64,
}

/// Runs either end of a copy on this machine.
pub(crate) async fn execute(
    client: &Arc<WorldClient>,
    call: &Call,
    session: &Arc<SessionContext>,
    token: &CancellationToken,
    timeout: Duration,
) -> VerbOutcome {
    match call.verb.as_str() {
        "copy" => source::run(client, call, session, token, timeout).await,
        _ => sink::accept(client, call, timeout),
    }
}

/// How the head note names one endpoint: bare on the caller's own machine, `node:path` on any
/// other.
pub(crate) fn endpoint(node: &str, path: &str, caller: &str) -> String {
    if node == caller {
        path.to_string()
    } else {
        format!("{node}:{path}")
    }
}

/// Rejects a relative path that could leave the destination directory or that the receiving
/// platform could not spell.
pub(crate) fn safe_relative(relative: &str) -> Result<Vec<String>, String> {
    let mut components = Vec::new();
    for component in relative.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".."
            || component.contains('\\')
            || component.contains(':')
            || component.contains('\0')
        {
            return Err(format!("the path \"{relative}\" cannot be written here"));
        }
        components.push(component.to_string());
    }
    Ok(components)
}
