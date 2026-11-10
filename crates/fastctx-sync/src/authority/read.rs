//! Read snapshots, access fences and bounded response accounting.
use super::{Peer, Service};
use crate::{
    catalog::Lifecycle,
    cursor::{Holder, HoldingPeriod},
    identity::ScopeKey,
    scope::{ScopeDescriptor, ScopeOwner},
    store::{Error, directory, scopes},
    transfer::Message,
};
use fastctx_storage::{ErrorKind, rusqlite::Connection};

pub(super) type Result<T> = std::result::Result<T, ReadError>;
pub(super) struct ReadError(pub Error);
impl From<Error> for ReadError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}
impl From<fastctx_storage::Error> for ReadError {
    fn from(error: fastctx_storage::Error) -> Self {
        Self(match error.kind() {
            ErrorKind::Busy => Error::Busy,
            ErrorKind::Full => Error::Full,
            ErrorKind::Corrupt => Error::Corrupt(error.to_string()),
            ErrorKind::Invalid => Error::Invalid(error.to_string()),
            _ => Error::Unavailable(error.to_string()),
        })
    }
}
impl From<fastctx_storage::rusqlite::Error> for ReadError {
    fn from(error: fastctx_storage::rusqlite::Error) -> Self {
        fastctx_storage::Error::from(error).into()
    }
}
impl From<serde_json::Error> for ReadError {
    fn from(error: serde_json::Error) -> Self {
        corrupt(error.to_string())
    }
}
pub(super) fn corrupt(reason: impl Into<String>) -> ReadError {
    Error::Corrupt(reason.into()).into()
}
pub(super) fn invalid(reason: impl Into<String>) -> ReadError {
    Error::Invalid(reason.into()).into()
}
pub(super) fn unavailable(reason: impl Into<String>) -> ReadError {
    Error::Unavailable(reason.into()).into()
}

impl Service {
    /// Metadata is read under one SQLite snapshot. A fresh authorization check
    /// before returning prevents a snapshot taken before revocation escaping it.
    pub(super) async fn read(
        &self,
        peer: Peer,
        scope: ScopeKey,
        holding: Option<HoldingPeriod>,
        query: impl FnOnce(&Connection, &ScopeDescriptor) -> Result<Message> + Send + 'static,
    ) -> std::result::Result<Message, Error> {
        if scope.hub != *self.directory.hub() {
            return Err(Error::Denied("scope is not readable by this peer".into()));
        }
        let (check_peer, check_scope, check_holding, policy) =
            (peer.clone(), scope.clone(), holding.clone(), self.policy.clone());
        let (answer, descriptor) = self
            .directory
            .database()
            .read(move |db| {
                Ok((|| -> Result<_> {
                    let snapshot = db.unchecked_transaction()?;
                    let descriptor = authorize(&snapshot, &peer, &scope, holding.as_ref(), policy.as_ref())?;
                    let answer = query(&snapshot, &descriptor)?;
                    snapshot.commit()?;
                    Ok((answer, descriptor))
                })())
            })
            .await
            .map_err(|error| ReadError::from(error).0)?
            .map_err(|error| error.0)?;
        let policy = self.policy.clone();
        self.directory
            .database()
            .read(move |db| {
                Ok((|| -> Result<()> {
                    let snapshot = db.unchecked_transaction()?;
                    let current =
                        authorize(&snapshot, &check_peer, &check_scope, check_holding.as_ref(), policy.as_ref())?;
                    if current != descriptor {
                        return Err(Error::Revoked.into());
                    }
                    snapshot.commit()?;
                    Ok(())
                })())
            })
            .await
            .map_err(|error| ReadError::from(error).0)?
            .map_err(|error| error.0)?;
        Ok(answer)
    }
}
pub(super) fn authorize(
    db: &Connection,
    peer: &Peer,
    scope: &ScopeKey,
    holding: Option<&HoldingPeriod>,
    policy: &dyn super::ReadPolicy,
) -> Result<ScopeDescriptor> {
    let denied = || ReadError(Error::Denied("scope is not readable by this peer".into()));
    let descriptor = directory::described(db, scope)?.ok_or_else(denied)?;
    let ScopeOwner::Account { identity } = &descriptor.owner else {
        return Err(denied());
    };
    if !peer.accounts.contains(&identity.id) || !policy.can_read(peer, &descriptor) {
        return Err(denied());
    }
    let current = scopes::current(db, scope)?.ok_or_else(denied)?;
    if !current.enabled || descriptor.lifecycle != Lifecycle::Live || descriptor.acl_revision != current.acl_revision {
        return Err(Error::Revoked.into());
    }
    if let Some(holding) = holding {
        if holding.scope != *scope || holding.holder != (Holder::Device { device: peer.device.clone() }) {
            return Err(denied());
        }
        if holding.acl_revision != current.acl_revision {
            return Err(Error::Revoked.into());
        }
        // Publication epochs belong to the receiving device, not to this hub's
        // own custody. Echo the authenticated device's tenure without comparing
        // it to the hub's independently advancing publication epoch.
    }
    Ok(descriptor)
}

/// Counts the actual JSON, including base64, counters and message envelopes,
/// without allocating another copy of the response.
pub(super) fn size(value: &impl serde::Serialize) -> Result<usize> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value)?;
    Ok(count.0)
}
pub(super) fn budget(bytes: u32) -> std::result::Result<usize, Error> {
    if bytes == 0 || bytes > fastctx_types::sync::PAGE_BYTES {
        return Err(Error::Invalid("metadata pages must fit the protocol page budget".into()));
    }
    Ok(bytes as usize)
}
