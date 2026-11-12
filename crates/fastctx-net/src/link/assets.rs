//! The second version's asset sync (`fastctx_types::assets`), asked of a
//! hub whose view declares the same `asset_sync`.

use super::Link;
use crate::wire::{Question, Told};
use fastctx_types::assets::{Reply, Request, VERSION};

impl Link {
    /// Asks the hub `request`; an error where it speaks another asset sync,
    /// or none.
    pub async fn assets(&self, request: Request) -> Result<Reply, String> {
        if self.view().asset_sync != VERSION {
            return Err(format!("the hub speaks asset sync {}, this machine {VERSION}", self.view().asset_sync));
        }
        match self.inner.ask(Question::Assets { request }).await? {
            Told::Assets { reply } => Ok(reply),
            _ => Err("the hub did not answer the asset request".into()),
        }
    }
}
