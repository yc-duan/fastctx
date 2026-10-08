use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_file_search::Search;
use fastctx_types::MachineName;
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

pub(crate) async fn call(
    node: &Arc<Node>,
    machine: Option<MachineName>,
    request: FileSearchRequest,
) -> Result<FileSearchReply, Failure> {
    let local = node.machine();
    let Some(target) = machine.filter(|machine| *machine != local) else {
        return here(node, local, request).await;
    };
    let link = node.link().ok_or_else(|| Failure::new(format!("{local} is not connected to any other machine")))?;
    match link.call(&target, RemoteCall::FindFiles { request }).await.map_err(failed)?? {
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
            let roots = roots(node, &scope).await?;
            let session = tokio::task::spawn_blocking(move || {
                let index = Search::new(roots.clone())?;
                Ok::<_, std::io::Error>(Session {
                    scope,
                    state: Mutex::new(SessionIndex { roots, index: Arc::new(index) }),
                })
            })
            .await
            .map_err(failed)?
            .map_err(failed)?;
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(failed)?;
            let search = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            node.file_searches
                .sessions
                .lock()
                .expect("searches poisoned")
                .insert((from, search.clone()), Arc::new(session));
            FileSearchResult::Started { search }
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
            let here = node.machine();
            Ok(record
                .places
                .into_iter()
                .filter(|place| place.location.machine == here)
                .map(|place| place.location.path)
                .collect())
        }
    }
}
