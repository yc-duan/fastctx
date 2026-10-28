//! Account record access; adapters provide authoritative comparison and change streams.

use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub(crate) type Reply<'a, T> = Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub version: u64,
    /// None is a durable deletion; JSON null is Some(Value::Null).
    pub body: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Exchanged {
    Written { version: u64 },
    Conflict { current: Option<u64> },
}

pub(crate) struct Changed {
    pub key: String,
    pub version: u64,
}

pub(crate) trait Changes: Send {
    /// A reconnect begins with the current keys, rather than assuming no change while disconnected.
    fn next(&mut self) -> Reply<'_, Vec<Changed>>;
}

pub(crate) trait Records: Send + Sync {
    fn available(&self) -> bool;
    fn read<'a>(&'a self, account: &'a HubAccount, kind: &'a str, key: &'a str) -> Reply<'a, Option<Record>>;
    fn list<'a>(&'a self, account: &'a HubAccount, kind: &'a str) -> Reply<'a, Vec<Changed>>;
    /// None matches a key with no version; a tombstone still requires its version.
    fn exchange<'a>(
        &'a self,
        account: &'a HubAccount,
        kind: &'a str,
        key: &'a str,
        expected: Option<u64>,
        body: Option<Value>,
    ) -> Reply<'a, Exchanged>;
    fn watch<'a>(&'a self, account: &'a HubAccount, kind: &'a str) -> Reply<'a, Box<dyn Changes>>;
}

pub(crate) struct Unconnected;

fn missing<T>() -> Reply<'static, T> {
    Box::pin(async { Err(Failure::new(super::words::RECORDS_MISSING)) })
}

impl Records for Unconnected {
    fn available(&self) -> bool {
        false
    }
    fn read<'a>(&'a self, _: &'a HubAccount, _: &'a str, _: &'a str) -> Reply<'a, Option<Record>> {
        missing()
    }
    fn list<'a>(&'a self, _: &'a HubAccount, _: &'a str) -> Reply<'a, Vec<Changed>> {
        missing()
    }
    fn exchange<'a>(
        &'a self,
        _: &'a HubAccount,
        _: &'a str,
        _: &'a str,
        _: Option<u64>,
        _: Option<Value>,
    ) -> Reply<'a, Exchanged> {
        missing()
    }
    fn watch<'a>(&'a self, _: &'a HubAccount, _: &'a str) -> Reply<'a, Box<dyn Changes>> {
        missing()
    }
}
