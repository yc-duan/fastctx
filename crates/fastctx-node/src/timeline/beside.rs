//! Where what is written beside a conversation's timeline goes — its timers
//! and its evidence folders among them — and whether this machine writes it
//! now: the scope is held here; one from elsewhere not before the hub
//! described it and every stream of it was pulled here once (AG10, MM02);
//! not one let go to another machine with its export (J08); and not while
//! another device orders the stream, which is then taken over (§28.10).

use super::assets::placed;
use super::serial::Held;
use crate::Node;
use fastctx_content::v2::Library;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::sync::Arc;

/// Where a stream beside a conversation's timeline is written.
pub(super) struct Beside {
    pub(super) library: Library,
    pub(super) scope: String,
    pub(super) holding: i64,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where `resident`'s `stream` goes, when this machine writes it now.
pub(super) async fn writable(
    node: &Arc<Node>,
    resident: &ResidentId,
    stream: &'static str,
    held: &Held,
) -> Result<Option<Beside>, Failure> {
    let Some(assets) = node.assets.as_ref() else { return Ok(None) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    let Some(placed) = placed(node, &about)? else { return Ok(None) };
    let (library, scope) = (assets.library.clone(), placed.scope.clone());
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &scope, &assets.device()) {
        return Ok(None);
    }
    let (asking, looked, holds) = (library.clone(), scope.clone(), held.clone());
    let (holding, live, orderer) = fastctx_platform::task::blocking(move || {
        let _held = holds;
        Ok::<_, fastctx_content::Error>((
            asking.holding(&looked)?,
            asking.is_live(&looked)? && asking.is_pulled(&looked)?,
            asking.orderer(&looked, stream)?,
        ))
    })
    .await
    .map_err(failure)?;
    let Some(holding) = holding else { return Ok(None) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !live {
        return Ok(None);
    }
    if orderer.is_some_and(|orderer| orderer != assets.device()) {
        crate::assets::takeover::want(node, &placed.hub, &scope, stream);
        return Ok(None);
    }
    Ok(Some(Beside { library, scope, holding }))
}
