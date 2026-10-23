//! Sentinel records are the account's records on its hub, reached through this
//! machine's link. A record the hub keeps only as deleted does not exist here.
use crate::Node;
use fastctx_net::link::Link;
use fastctx_sentinel::records::{Change, Error, Head, Record, Records, ResultFuture};
use fastctx_types::access::HubAccount;
use fastctx_types::account_records::{Request, Response, Watched};
use serde_json::Value;
use std::sync::{Arc, Weak};
use tokio::sync::broadcast;

fn unexpected() -> Error {
    Error::Unavailable("the hub answered another account-records reply".into())
}

pub(crate) struct AccountRecords(pub Weak<Node>);
impl AccountRecords {
    fn link(&self, scope: &HubAccount) -> Result<Arc<Link>, Error> {
        let node = self.0.upgrade().ok_or_else(|| Error::Unavailable("the sentinel node has stopped".into()))?;
        node.link_of(&scope.hub).ok_or_else(|| Error::Unavailable("the account's network is not connected".into()))
    }
    async fn ask(&self, scope: &HubAccount, request: Request) -> Result<Response, Error> {
        self.link(scope)?.account_records(&scope.account, request).await.map_err(Error::Unavailable)
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
    fn list<'a>(&'a self, scope: &'a HubAccount, kind: &'a str) -> ResultFuture<'a, Vec<Head>> {
        Box::pin(async move {
            match self.ask(scope, Request::List { kind: kind.into() }).await? {
                Response::Listed { entries } => {
                    Ok(entries.into_iter().map(|entry| Head { key: entry.key, version: entry.version }).collect())
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
                        return Ok(value.map(|value| Record { version: metadata.version, value }));
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
            let mut watch =
                self.link(scope)?.watch_account_records(scope.account.clone(), kind.into()).map_err(Error::Unavailable)?;
            let (sender, receiver) = broadcast::channel(64);
            // Ending closes the receiver, and the sentinel subscribes and lists again.
            tokio::spawn(async move {
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
                        Ok(Watched::Changed { metadata }) => vec![Change {
                            key: metadata.key,
                            version: (!metadata.deleted).then_some(metadata.version),
                        }],
                        Err(_) => return,
                    };
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
