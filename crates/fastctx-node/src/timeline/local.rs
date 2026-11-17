//! A conversation's timeline read from the asset library (design §28.3,
//! plan 3-5): once the library holds the conversation's timeline — made
//! here, or come made from elsewhere — a reader reads it there, at once,
//! however long the history; before that, and for a conversation on no
//! network, from the content timeline (`reads`). Reads only read: nothing
//! is produced for them.

use crate::Node;
use fastctx_content::v2::{Kind as _, Timeline};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where a conversation's timeline is read in the asset library.
pub(crate) use fastctx_client::timeline_copy::Copy as Held;
pub(crate) use fastctx_client::timeline_copy::{changes, moved, object, page};

/// The conversation's timeline in the asset library, when a reader reads it
/// there: its scope held here, and its timeline made — by this machine's
/// producer at least once, or come from the device that orders it.
pub(crate) fn held(node: &Node, resident: &ResidentId) -> Result<Option<Held>, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    let Some(placed) = super::assets::placed(node, &about)? else { return Ok(None) };
    let library = kept.library.clone();
    let Some(at) = library.view_revision(&placed.scope, Timeline::STREAM).map_err(failure)? else { return Ok(None) };
    // Made here only as the producer last finished making it whole in the
    // holding period the scope is in now: one of an earlier period, or
    // set back as a whole making began, is not (JJ05).
    let holding = library.holding(&placed.scope).map_err(failure)?;
    let made_here = library
        .checkpoint(&super::assets::checkpoint(resident))
        .map_err(failure)?
        .is_some_and(|state| holding.is_some() && super::tail::made_in(&state) == holding);
    let made_elsewhere = at & ((1 << 40) - 1) > 0
        && library
            .orderer(&placed.scope, Timeline::STREAM)
            .map_err(failure)?
            .is_some_and(|orderer| orderer != kept.device());
    Ok((made_here || made_elsewhere).then(|| Held::at(library, placed.scope)))
}
