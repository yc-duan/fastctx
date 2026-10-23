//! Account-record port supplied by the hub's record service.
use fastctx_types::access::HubAccount;
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug)]
pub struct Record {
    pub version: u64,
    pub value: Value,
}
#[derive(Clone, Debug)]
pub struct Head {
    pub key: String,
    pub version: u64,
}
#[derive(Clone, Debug)]
pub struct Change {
    pub key: String,
    pub version: Option<u64>,
}
#[derive(Clone, Debug)]
pub enum Error {
    Conflict { current_version: Option<u64> },
    Unavailable(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { current_version } => write!(f, "record version changed to {current_version:?}"),
            Self::Unavailable(reason) => f.write_str(reason),
        }
    }
}
pub type ResultFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Reads and lists an account's records; writes and deletes are compare-exchange.
/// Subscribe before listing; a lagged watcher must relist. The provider enforces
/// account isolation and delivers changes only after the write is durable.
pub trait Records: Send + Sync {
    fn get<'a>(&'a self, scope: &'a HubAccount, kind: &'a str, key: &'a str) -> ResultFuture<'a, Option<Record>>;
    fn list<'a>(&'a self, scope: &'a HubAccount, kind: &'a str) -> ResultFuture<'a, Vec<Head>>;
    fn compare_exchange<'a>(
        &'a self,
        scope: &'a HubAccount,
        kind: &'a str,
        key: &'a str,
        expected: Option<u64>,
        value: Option<Value>,
    ) -> ResultFuture<'a, Option<Record>>;
    fn watch<'a>(
        &'a self,
        scope: &'a HubAccount,
        kind: &'a str,
    ) -> ResultFuture<'a, tokio::sync::broadcast::Receiver<Change>>;
}

/// An unconnected integration seam is an explicit failure, never a local copy
/// presented as the account's authoritative record.
pub struct Unavailable;
impl Records for Unavailable {
    fn get<'a>(&'a self, _: &'a HubAccount, _: &'a str, _: &'a str) -> ResultFuture<'a, Option<Record>> {
        Box::pin(async { Err(Error::Unavailable("account record service is not connected".into())) })
    }
    fn list<'a>(&'a self, _: &'a HubAccount, _: &'a str) -> ResultFuture<'a, Vec<Head>> {
        Box::pin(async { Err(Error::Unavailable("account record service is not connected".into())) })
    }
    fn compare_exchange<'a>(
        &'a self,
        _: &'a HubAccount,
        _: &'a str,
        _: &'a str,
        _: Option<u64>,
        _: Option<Value>,
    ) -> ResultFuture<'a, Option<Record>> {
        Box::pin(async { Err(Error::Unavailable("account record service is not connected".into())) })
    }
    fn watch<'a>(
        &'a self,
        _: &'a HubAccount,
        _: &'a str,
    ) -> ResultFuture<'a, tokio::sync::broadcast::Receiver<Change>> {
        Box::pin(async { Err(Error::Unavailable("account record service is not connected".into())) })
    }
}
