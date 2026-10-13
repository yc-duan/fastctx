use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_file_search::Search;
use fastctx_types::MachineName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::file_search::{FileSearchReply, FileSearchRequest, FileSearchResult, FileSearchScope};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

struct Session {
    scope: FileSearchScope,
    state: Mutex<SessionIndex>,
    closed: tokio::sync::watch::Sender<bool>,
}

struct SessionIndex {
    roots: Vec<String>,
    index: Arc<Search>,
}

type Sessions = HashMap<(MachineName, String), Arc<Session>>;

#[derive(Default)]
pub(crate) struct Searches {
    sessions: Mutex<Sessions>,
}

pub(crate) enum Lease {
    Local {
        node: Arc<Node>,
        from: MachineName,
        search: String,
        ended: tokio::sync::watch::Receiver<bool>,
        close_on_drop: bool,
    },
    Remote(fastctx_net::file_search::Lease),
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Self::Local { node, from, search, close_on_drop: true, .. } = self {
            let session =
                node.file_searches.sessions.lock().expect("searches poisoned").remove(&(from.clone(), search.clone()));
            tokio::task::spawn_blocking(move || drop(session));
        }
    }
}

impl Lease {
    pub async fn closed(&mut self) {
        match self {
            Self::Local { ended, .. } => {
                let _ = ended.wait_for(|closed| *closed).await;
            }
            Self::Remote(lease) => lease.closed().await,
        }
    }
}

pub(crate) async fn open(
    node: &Arc<Node>,
    acting: &Acting,
    from: MachineName,
    machine: Option<MachineName>,
    scope: FileSearchScope,
) -> Result<(FileSearchReply, Lease), Failure> {
    if let Some(machine) = machine.filter(|machine| !node.is_me(machine)) {
        let (link, account) = node
            .speaking(acting, &(&machine).into(), Purpose::Hands)
            .map_err(|failure| Failure::new(failure.to_string()))?;
        let (reply, lease) = fastctx_net::file_search::open(link, machine, account, scope).await?;
        return Ok((reply, Lease::Remote(lease)));
    }
    let roots = roots(node, &scope).await?;
    let session = tokio::task::spawn_blocking(move || {
        let index = Search::new(roots.clone())?;
        Ok::<_, std::io::Error>(Session {
            scope,
            state: Mutex::new(SessionIndex { roots, index: Arc::new(index) }),
            closed: tokio::sync::watch::channel(false).0,
        })
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(failed)?;
    let search = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let ended = session.closed.subscribe();
    let lease =
        Lease::Local { node: node.clone(), from: from.clone(), search: search.clone(), ended, close_on_drop: true };
    node.file_searches.sessions.lock().expect("searches poisoned").insert((from, search.clone()), Arc::new(session));
    let reply = FileSearchReply { machine: node.machine(), result: FileSearchResult::Started { search } };
    Ok((reply, lease))
}

pub(crate) async fn serve(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    from: MachineName,
    scope: FileSearchScope,
    mut stream: fastctx_net::data::Stream,
) -> Result<RemoteReply, Failure> {
    let (reply, mut lease) = open(node, &Acting::Here, from, None, scope).await?;
    stream.say(&link.sign(fastctx_net::sign::Domain::Event, &reply)).await.map_err(failed)?;
    tokio::select! { () = stream.closed() => {}, () = lease.closed() => {} }
    Ok(RemoteReply::FilesFound(FileSearchReply { machine: node.machine(), result: FileSearchResult::Closed }))
}

pub(crate) async fn call(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<MachineName>,
    request: FileSearchRequest,
) -> Result<FileSearchReply, Failure> {
    let local = node.machine();
    let Some(target) = machine.filter(|machine| *machine != local) else {
        return here(node, local, request).await;
    };
    let call = RemoteCall::FindFiles { request };
    match node.call(acting, &(&target).into(), Purpose::Hands, call).await.map_err(failed)?? {
        RemoteReply::FilesFound(reply) => Ok(reply),
        _ => Err(Failure::new(format!("{target} answered something other than a file search"))),
    }
}

pub(crate) async fn here(
    node: &Arc<Node>,
    from: MachineName,
    request: FileSearchRequest,
) -> Result<FileSearchReply, Failure> {
    let result = match request {
        FileSearchRequest::Start { scope } => {
            let (reply, mut lease) = open(node, &Acting::Here, from, None, scope).await?;
            if let Lease::Local { close_on_drop, .. } = &mut lease {
                *close_on_drop = false;
            }
            return Ok(reply);
        }
        FileSearchRequest::Open { .. } => {
            return Err(Failure::new("opening a file search requires its owning connection"));
        }
        FileSearchRequest::Query { search, query, limit } => {
            let session = node
                .file_searches
                .sessions
                .lock()
                .expect("searches poisoned")
                .get(&(from, search))
                .cloned()
                .ok_or_else(|| Failure::new("file search is closed or belongs to another caller"))?;
            let scope = session.scope.clone();
            let current = roots(node, &scope).await?;
            let expected = current.clone();
            let original_query = query.clone();
            let result = tokio::task::spawn_blocking(move || {
                let index = {
                    let mut state = session.state.lock().expect("search session poisoned");
                    if state.roots != current {
                        state.index = Arc::new(Search::new(current.clone())?);
                        state.roots = current;
                    }
                    state.index.clone()
                };
                index.query(query, limit)
            })
            .await
            .map_err(failed)?
            .map_err(failed)?;
            if roots(node, &scope).await? != expected {
                FileSearchResult::Matches { query: original_query, ready: false, total: 0, files: vec![] }
            } else {
                result
            }
        }
        FileSearchRequest::Close { search } => {
            let session = node.file_searches.sessions.lock().expect("searches poisoned").remove(&(from, search));
            if let Some(session) = &session {
                session.closed.send_replace(true);
            }
            tokio::task::spawn_blocking(move || drop(session)).await.map_err(failed)?;
            FileSearchResult::Closed
        }
    };
    Ok(FileSearchReply { machine: node.machine(), result })
}

async fn roots(node: &Arc<Node>, scope: &FileSearchScope) -> Result<Vec<String>, Failure> {
    match scope {
        FileSearchScope::Roots { roots } => Ok(roots.clone()),
        FileSearchScope::Resident { resident, roots } => {
            let town = node
                .residents
                .town_of(resident)
                .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
            let Some(town) = town else {
                return Ok(roots.clone());
            };
            let world = node.world.clone();
            let record = tokio::task::spawn_blocking(move || crate::town::read::latest(&world, &town))
                .await
                .map_err(failed)?
                .map_err(failed)?
                .ok_or_else(|| Failure::new("the resident's project has no local address book"))?;
            Ok(record
                .places
                .into_iter()
                .filter(|place| node.is_me(&place.location.machine))
                .map(|place| place.location.path)
                .collect())
        }
    }
}
