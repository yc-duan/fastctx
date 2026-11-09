//! A hub's side of asset sync: one service answers each authenticated device's
//! requests from the hub's engines. The transport builds the [`Peer`] from the
//! authenticated connection; nothing in a request grants a permission.
use crate::{
    identity::{AccountId, DeviceId},
    store::{
        Error, FutureResult,
        directory::{Recipient, SqliteDirectory},
    },
    transfer::Message,
};
use std::sync::Arc;

/// The device a request comes from and the accounts it serves on this hub, as
/// the transport established them.
#[derive(Clone, Debug)]
pub struct Peer {
    pub device: DeviceId,
    pub accounts: Vec<AccountId>,
}

pub struct Service {
    directory: Arc<SqliteDirectory>,
}

impl Service {
    pub fn new(directory: Arc<SqliteDirectory>) -> Self {
        Self { directory }
    }

    /// Answers one metadata request from `peer`.
    pub fn answer<'a>(&'a self, peer: &'a Peer, request: Message) -> FutureResult<'a, Message> {
        Box::pin(async move {
            match request {
                Message::GetScopes { request } => {
                    let recipient = Recipient { device: peer.device.clone(), accounts: peer.accounts.clone() };
                    Ok(Message::ScopesPage { page: self.directory.discover(recipient, request).await? })
                }
                _ => Err(Error::Unavailable("this hub does not serve that request yet".into())),
            }
        })
    }
}
