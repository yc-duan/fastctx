//! Whether a conversation was compacted since its host last answered, as
//! the host's own record shows (`fastctx_hosts::claude::compacted`): by the
//! user, by the host itself or by anything else. A Claude Code compaction is
//! no turn: it leaves the conversation's last turn, and the time it has been
//! idle since, as they were, and the context heard of it was counted before
//! it. Such a stretch has nothing left for one more compaction to save. A
//! Codex compaction is a turn of its own that says nothing, which `idle`
//! already lets be.

use fastctx_hosts::claude::compacted::{self, Last};
use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;

use crate::Node;

/// Why `id` is not compacted now: its record says it was compacted since
/// its host last answered, or cannot say either way.
pub(super) fn since_answer(node: &Node, id: &ResidentId) -> Result<(), String> {
    let Some((about, _)) = node.residents.about(id) else { return Err("it is no longer on record".into()) };
    if about.host.agent.as_str() != "claude" {
        return Ok(());
    }
    let Some(session) = about.session.as_deref() else {
        return Err("Claude has not said which session it is".into());
    };
    let path = match exchange::transcript(&exchange::sessions(node.world.path()).join(session)) {
        Ok(Some(path)) => path,
        Ok(None) => return Err("Claude has not said where its record is".into()),
        Err(error) => return Err(format!("reading where Claude keeps its record: {error}")),
    };
    match compacted::last(&path) {
        Ok(Some(Last::Answer)) => Ok(()),
        Ok(Some(Last::Compaction)) => Err("it was compacted since its host last answered".into()),
        Ok(None) => Err("the end of its record shows neither an answer nor a compaction".into()),
        Err(error) => Err(format!("reading the end of its record: {error}")),
    }
}
