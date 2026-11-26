//! Sentinel records are the account's records on its hub, reached through this
//! machine's link. A record the hub keeps only as deleted does not exist here.
use crate::Node;
use fastctx_net::link::Link;
use fastctx_sentinel::records::{Change, Error, Head, Record, Records, ResultFuture};
use fastctx_types::access::HubAccount;
use fastctx_types::account_records::{Request, Response, Watched};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::broadcast;

fn unexpected() -> Error {
    Error::Unavailable("the hub answered another account-records reply".into())
}

/// The records of an account, with the last read of each that a listing
/// named (`get_listed`): read again only once a listing names another
/// version of it.
pub(crate) struct AccountRecords {
    node: Weak<Node>,
    kept: Mutex<HashMap<(HubAccount, String, String), Record>>,
}
impl AccountRecords {
    pub(crate) fn new(node: Weak<Node>) -> Self {
        Self { node, kept: Mutex::default() }
    }

    fn keep(&self, scope: &HubAccount, kind: &str, key: &str, record: Option<&Record>) {
        let mut kept = self.kept.lock().expect("never poisoned");
        let at = (scope.clone(), kind.to_string(), key.to_string());
        match record {
            Some(record) => {
                kept.insert(at, record.clone());
            }
            None => {
                kept.remove(&at);
            }
        }
    }

    fn link(&self, scope: &HubAccount) -> Result<Arc<Link>, Error> {
        let node = self.node.upgrade().ok_or_else(|| Error::Unavailable("the sentinel node has stopped".into()))?;
        node.link_of(&scope.hub).ok_or_else(|| Error::Unavailable("the account's network is not connected".into()))
    }
    async fn ask(&self, scope: &HubAccount, request: Request) -> Result<Response, Error> {
        let link = self.link(scope)?;
        crate::record_access::ask(&link, scope, request).await.map_err(|failure| Error::Unavailable(failure.message))
    }
}
impl Records for AccountRecords {
    fn get<'a>(&'a self, scope: &'a HubAccount, kind: &'a str, key: &'a str) -> ResultFuture<'a, Option<Record>> {
        Box::pin(async move {
            match self.ask(scope, Request::Read { kind: kind.into(), key: key.into() }).await? {
                Response::Read { record: Some(record) } if !record.metadata.deleted => {
                    Ok(Some(Record { version: record.metadata.version, value: record.body }))
                }
                Response::Read { .. } => Ok(None),
                _ => Err(unexpected()),
            }
        })
    }
    fn get_listed<'a>(
        &'a self,
        scope: &'a HubAccount,
        kind: &'a str,
        head: &'a Head,
    ) -> ResultFuture<'a, Option<Record>> {
        Box::pin(async move {
            let at = (scope.clone(), kind.to_string(), head.key.clone());
            let kept =
                self.kept.lock().expect("never poisoned").get(&at).filter(|kept| kept.version == head.version).cloned();
            if let Some(record) = kept {
                return Ok(Some(record));
            }
            let record = self.get(scope, kind, &head.key).await?;
            self.keep(scope, kind, &head.key, record.as_ref());
            Ok(record)
        })
    }
    fn list<'a>(&'a self, scope: &'a HubAccount, kind: &'a str) -> ResultFuture<'a, Vec<Head>> {
        Box::pin(async move {
            match self.ask(scope, Request::List { kind: kind.into() }).await? {
                Response::Listed { entries } => {
                    let heads: Vec<Head> =
                        entries.into_iter().map(|entry| Head { key: entry.key, version: entry.version }).collect();
                    // What the listing no longer names is not kept.
                    let named: std::collections::HashSet<&str> = heads.iter().map(|head| head.key.as_str()).collect();
                    self.kept
                        .lock()
                        .expect("never poisoned")
                        .retain(|(at, of, key), _| at != scope || of != kind || named.contains(key.as_str()));
                    Ok(heads)
                }
                _ => Err(unexpected()),
            }
        })
    }
    fn compare_exchange<'a>(
        &'a self,
        scope: &'a HubAccount,
        kind: &'a str,
        key: &'a str,
        expected: Option<u64>,
        value: Option<Value>,
    ) -> ResultFuture<'a, Option<Record>> {
        Box::pin(async move {
            let mut expected = expected;
            let mut over_deleted = false;
            loop {
                let request = match &value {
                    Some(body) => Request::Write { kind: kind.into(), key: key.into(), expected, body: body.clone() },
                    None => Request::Delete { kind: kind.into(), key: key.into(), expected },
                };
                match self.ask(scope, request).await? {
                    Response::Written { metadata } => {
                        if let Some(node) = self.node.upgrade() {
                            node.topics.changed(fastctx_types::topics::SENTINELS);
                        }
                        let record = value.map(|value| Record { version: metadata.version, value });
                        self.keep(scope, kind, key, record.as_ref());
                        return Ok(record);
                    }
                    // Writing over what was deleted names the version the hub kept.
                    Response::Conflict { current: Some(current) }
                        if current.deleted && expected.is_none() && value.is_some() && !over_deleted =>
                    {
                        expected = Some(current.version);
                        over_deleted = true;
                    }
                    Response::Conflict { current } => {
                        return Err(Error::Conflict {
                            current_version: current.filter(|current| !current.deleted).map(|current| current.version),
                        });
                    }
                    _ => return Err(unexpected()),
                }
            }
        })
    }
    fn watch<'a>(&'a self, scope: &'a HubAccount, kind: &'a str) -> ResultFuture<'a, broadcast::Receiver<Change>> {
        Box::pin(async move {
            let mut watch = self
                .link(scope)?
                .watch_account_records(scope.account.clone(), kind.into())
                .map_err(Error::Unavailable)?;
            let (sender, receiver) = broadcast::channel(64);
            let node = self.node.clone();
            // Ending closes the receiver, and the sentinel subscribes and lists again.
            fastctx_platform::task::spawn(async move {
                loop {
                    let next = tokio::select! {
                        next = watch.next() => next,
                        () = sender.closed() => return,
                    };
                    let changes = match next {
                        Ok(Watched::Current { entries }) => entries
                            .into_iter()
                            .map(|entry| Change { key: entry.key, version: Some(entry.version) })
                            .collect(),
                        Ok(Watched::Changed { metadata }) => {
                            vec![Change { key: metadata.key, version: (!metadata.deleted).then_some(metadata.version) }]
                        }
                        Err(_) => return,
                    };
                    if let Some(node) = node.upgrade() {
                        node.topics.changed(fastctx_types::topics::SENTINELS);
                    }
                    for change in changes {
                        if sender.send(change).is_err() {
                            return;
                        }
                    }
                }
            });
            Ok(receiver)
        })
    }
}
