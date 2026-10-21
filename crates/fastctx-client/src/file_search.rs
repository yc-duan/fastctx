use fastctx_types::access::Written;
use fastctx_types::file_search::{FileSearchReply, FileSearchRequest, FileSearchResult, FileSearchScope};
use fastctx_types::{Call, Reply};
use std::sync::Arc;

use crate::{Error, World};

pub(crate) struct Searches {
    generation: u64,
    closed: tokio::sync::watch::Sender<()>,
    held: std::collections::HashMap<(Written, String), Arc<Search>>,
}

impl Default for Searches {
    fn default() -> Self {
        Self { generation: 0, closed: tokio::sync::watch::channel(()).0, held: Default::default() }
    }
}
enum Held {
    Local { _client: crate::Client },
    Remote { _lease: fastctx_net::file_search::Lease },
}

struct Opened {
    machine: Written,
    search: String,
    _held: Held,
}

struct Search {
    scope: FileSearchScope,
    closed: tokio::sync::watch::Sender<bool>,
    current: tokio::sync::Mutex<Opened>,
}

fn closed() -> Error {
    Error::Unexpected("the page owning this file search closed".into())
}

impl World {
    /// Start for the conversation's composer on `machine`, before its first
    /// `@`. Query on edits and while indexing, retain across popovers, and
    /// close when leaving the composer. Use the returned search
    /// id only with the same machine and World identity. Devices must name
    /// a machine; that machine checks their remote permission.
    pub async fn find_files(
        &self,
        machine: Option<Written>,
        request: FileSearchRequest,
    ) -> Result<FileSearchReply, Error> {
        if let FileSearchRequest::Start { scope } | FileSearchRequest::Open { scope } = request {
            let (generation, mut closed) = {
                let searches = self.searches.lock().expect("file searches");
                (searches.generation, searches.closed.subscribe())
            };
            let (reply, opened) = tokio::select! {
                result = self.open_search(machine, scope.clone()) => result?,
                _ = closed.changed() => return Err(Error::Unexpected("the page owning this file search closed".into())),
            };
            let mut searches = self.searches.lock().expect("file searches");
            if searches.generation != generation {
                return Err(Error::Unexpected("the page owning this file search closed".into()));
            }
            searches.held.insert(
                (opened.machine.clone(), opened.search.clone()),
                Arc::new(Search {
                    scope,
                    closed: tokio::sync::watch::channel(false).0,
                    current: tokio::sync::Mutex::new(opened),
                }),
            );
            return Ok(reply);
        }
        let id = match &request {
            FileSearchRequest::Query { search, .. } | FileSearchRequest::Close { search } => search,
            _ => unreachable!("open returned above"),
        };
        let owned = {
            let mut searches = self.searches.lock().expect("file searches");
            let key = searches
                .held
                .keys()
                .find(|(host, held)| held == id && machine.as_ref().is_none_or(|machine| machine == host))
                .cloned();
            key.and_then(|key| {
                if matches!(request, FileSearchRequest::Close { .. }) {
                    let owned = searches.held.remove(&key)?;
                    owned.closed.send_replace(true);
                    Some(owned)
                } else {
                    searches.held.get(&key).cloned()
                }
            })
        };
        if let Some(owned) = owned {
            match request {
                FileSearchRequest::Query { query, limit, .. } => {
                    let mut ending = owned.closed.subscribe();
                    if *ending.borrow() {
                        return Err(closed());
                    }
                    let querying = async {
                        let mut current = owned.current.lock().await;
                        let answer = self
                            .query_search(current.machine.clone(), current.search.clone(), query.clone(), limit)
                            .await;
                        if matches!(&answer, Err(Error::Call(crate::CallError::Failed(failure)))
                            if failure.message == "file search is closed or belongs to another caller")
                        {
                            // The page is still here, but its previous connection/index ended.
                            // Preserve its public ID while replacing that expendable index.
                            let (_, opened) =
                                self.open_search(Some(current.machine.clone()), owned.scope.clone()).await?;
                            *current = opened;
                            self.query_search(current.machine.clone(), current.search.clone(), query, limit).await
                        } else {
                            answer
                        }
                    };
                    return tokio::select! {
                        result = querying => result,
                        _ = ending.changed() => Err(closed()),
                    };
                }
                FileSearchRequest::Close { .. } => {
                    let (machine, search) = {
                        let current = owned.current.lock().await;
                        (current.machine.clone(), current.search.clone())
                    };
                    drop(owned);
                    return self.search_call(Some(machine), FileSearchRequest::Close { search }).await;
                }
                _ => unreachable!("open returned above"),
            }
        }
        self.search_call(machine, request).await
    }

    async fn open_search(
        &self,
        machine: Option<Written>,
        scope: FileSearchScope,
    ) -> Result<(FileSearchReply, Opened), Error> {
        let selected = machine.clone();
        let (reply, held) = if let Some(device) = self.device() {
            let (reply, lease) = device.open_file_search(machine, scope).await?;
            (reply, Held::Remote { _lease: lease })
        } else {
            let mut client = self.connect().await?;
            let reply = client.call(Call::FindFiles { machine, request: FileSearchRequest::Open { scope } }).await?;
            let Reply::FilesFound(reply) = reply else { return Err(Error::Unexpected(format!("{reply:?}"))) };
            (reply, Held::Local { _client: client })
        };
        let FileSearchResult::Started { search } = &reply.result else {
            return Err(Error::Unexpected(format!("{reply:?}")));
        };
        let opened = Opened {
            machine: selected.unwrap_or_else(|| reply.machine.clone().into()),
            search: search.clone(),
            _held: held,
        };
        Ok((reply, opened))
    }

    async fn query_search(
        &self,
        machine: Written,
        search: String,
        query: String,
        limit: usize,
    ) -> Result<FileSearchReply, Error> {
        self.search_call(Some(machine), FileSearchRequest::Query { search, query, limit }).await
    }

    async fn search_call(
        &self,
        machine: Option<Written>,
        request: FileSearchRequest,
    ) -> Result<FileSearchReply, Error> {
        match self.call(Call::FindFiles { machine, request }).await?.0 {
            Reply::FilesFound(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Ends searches owned by an interface that is closing or replacing its page.
    pub fn close_file_searches(&self) {
        let mut searches = self.searches.lock().expect("file searches");
        searches.generation += 1;
        searches.closed.send_replace(());
        for (_, search) in searches.held.drain() {
            search.closed.send_replace(true);
        }
    }
}
