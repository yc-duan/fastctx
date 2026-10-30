mod sessions;

use fastctx_types::access::Written;
use std::sync::Arc;

pub(crate) use sessions::Searches;
use sessions::Session;

use fastctx_types::MachineName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::file_search::{FileSearchReply, FileSearchRequest, FileSearchResult, FileSearchScope};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) enum Lease {
    Local(sessions::Lease),
    Remote(fastctx_net::file_search::Lease),
}

impl Lease {
    pub async fn closed(&mut self) {
        match self {
            Self::Local(lease) => lease.closed().await,
            Self::Remote(lease) => lease.closed().await,
        }
    }
}

pub(crate) async fn open(
    node: &Arc<Node>,
    acting: &Acting,
    from: MachineName,
    machine: Option<Written>,
    scope: FileSearchScope,
) -> Result<(FileSearchReply, Lease), Failure> {
    if let Some(machine) = machine.filter(|machine| !node.is_here(machine)) {
        let (link, account) =
            node.speaking(acting, &machine, Purpose::Hands).map_err(|failure| Failure::new(failure.to_string()))?;
        let (reply, lease) = fastctx_net::file_search::open(link, machine.name, account, scope).await?;
        return Ok((reply, Lease::Remote(lease)));
    }
    node.authorize_here(acting)?;
    let initializing = crate::initializing::take().await?;
    let roots = roots(node, &scope, Some(&initializing)).await?;
    let session = initializing.run(move || Session::new(scope, roots)).await?.map_err(failed)?;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(failed)?;
    let search = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let lease = Lease::Local(node.file_searches.insert((from, search.clone()), session));
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
    let opening = open(node, &Acting::Here, from, None, scope);
    tokio::pin!(opening);
    let (reply, mut lease) = tokio::select! {
        result = &mut opening => result?,
        () = stream.closed() => {
            drop(opening.await);
            return Err(failed("the file search's owner disconnected"));
        },
    };
    stream.say(&link.sign(fastctx_net::sign::Domain::Event, &reply)).await.map_err(failed)?;
    tokio::select! { () = stream.closed() => {}, () = lease.closed() => {} }
    Ok(RemoteReply::FilesFound(FileSearchReply { machine: node.machine(), result: FileSearchResult::Closed }))
}

pub(crate) async fn call(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<Written>,
    request: FileSearchRequest,
) -> Result<FileSearchReply, Failure> {
    let local = node.machine();
    let Some(target) = machine.filter(|machine| !node.is_here(machine)) else {
        node.authorize_here(acting)?;
        return here(node, local, request).await;
    };
    let call = RemoteCall::FindFiles { request };
    match node.call(acting, &target, Purpose::Hands, call).await.map_err(failed)?? {
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
            let (reply, lease) = open(node, &Acting::Here, from, None, scope).await?;
            let Lease::Local(lease) = lease else { unreachable!("opened on this node") };
            lease.keep_until_idle();
            return Ok(reply);
        }
        FileSearchRequest::Open { .. } => {
            return Err(Failure::new("opening a file search requires its owning connection"));
        }
        FileSearchRequest::Query { search, query, limit } => {
            let session = node.file_searches.get(&(from, search))?;
            let scope = session.scope.clone();
            let current = roots(node, &scope, None).await?;
            let expected = current.clone();
            let original_query = query.clone();
            let result =
                tokio::task::spawn_blocking(move || session.query(current, query, limit)).await.map_err(failed)?;
            if roots(node, &scope, None).await? != expected {
                FileSearchResult::Matches { query: original_query, ready: false, total: 0, files: vec![] }
            } else {
                result.map_err(failed)?
            }
        }
        FileSearchRequest::Close { search } => {
            if let Some(closing) = node.file_searches.close(&(from, search)) {
                closing.await.map_err(failed)?;
            }
            FileSearchResult::Closed
        }
    };
    Ok(FileSearchReply { machine: node.machine(), result })
}

async fn roots(
    node: &Arc<Node>,
    scope: &FileSearchScope,
    initializing: Option<&crate::initializing::Guard>,
) -> Result<Vec<String>, Failure> {
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
            let name = town.clone();
            let reading = move || crate::town::read::latest(&world, &name);
            let record = match initializing {
                Some(initializing) => initializing.run(reading).await?,
                None => tokio::task::spawn_blocking(reading).await.map_err(failed)?,
            }
            .map_err(failed)?
            .ok_or_else(|| Failure::new("the resident's project has no local address book"))?;
            let mut roots = Vec::new();
            for place in record.places {
                let machine = crate::town::machines::destination(node, &town, &place.location.machine)?;
                if node.is_here(&machine) {
                    roots.push(place.location.path);
                }
            }
            Ok(roots)
        }
    }
}
