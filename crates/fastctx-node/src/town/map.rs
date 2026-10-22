//! A town's map (`world-map.md`): made from its record, its journals and
//! its canon each time it is asked for, never kept.

use std::io;
use std::path::Path;
use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_types::TownName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::project::{CanonHead, Location, MapView, Stamp};
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;
use crate::town::read;

/// The map of `town`, from this machine's copy of its ledger.
pub async fn view(node: &Arc<Node>, town: &TownName, acting: &Acting) -> Result<MapView, Failure> {
    super::access::ready(node, acting, town).await?;
    let (world, name) = (node.world.clone(), town.clone());
    let (records, journals, pages) = tokio::task::spawn_blocking(move || {
        let records = read::records(&world, &name)?;
        let journals = read::summaries(&world, &name)?;
        let pages = crate::pages::read::summaries(&world, &name)?;
        io::Result::Ok((records, journals, pages))
    })
    .await
    .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?
    .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?;
    let (Some(first), Some(record)) = (records.first(), records.last()) else {
        return Err(no_ledger(node, town));
    };
    let opened = Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
    let mut record = record.clone();
    crate::ledger::query::seen(node, acting, town, &mut record).map_err(|error| Failure::new(error.to_string()))?;
    let canon = match &record.canon {
        Some(location) => Some(canon(node, town, location, acting).await),
        None => None,
    };
    Ok(MapView {
        town: town.clone(),
        ledger: crate::world::native(&node.world.town(town).ledger()),
        record,
        opened,
        journals,
        pages,
        canon,
    })
}

/// What a resident living in `town` and working for `acting` is told of
/// it: its record, naming only the machines it may work on, while project
/// memory is not wired in.
pub async fn brief(node: &Arc<Node>, town: &TownName, acting: &Acting) -> Result<String, Failure> {
    super::access::ready(node, acting, town).await?;
    super::instructions::text(node, town, acting)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| no_ledger(node, town))
}

/// Reads a replicated town locally. Other towns are queried while the
/// connection is available, or read from an existing copy while offline.
pub async fn query(node: &Arc<Node>, town: &TownName) -> Result<(), Failure> {
    let towns = crate::ledger::query(node, Some(town))
        .await
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?;
    if towns.contains(town) { Ok(()) } else { Err(no_ledger(node, town)) }
}

/// Why there is no map of a town here.
pub fn no_ledger(node: &Node, town: &TownName) -> Failure {
    Failure::new(format!("{} keeps no ledger of a town {town}: no record of that town was found", node.machine()))
}

/// The documents of a canon, read where it is.
async fn canon(
    node: &Arc<Node>,
    town: &TownName,
    location: &Location,
    acting: &Acting,
) -> Result<Vec<CanonHead>, String> {
    let machine = super::machines::destination(node, town, &location.machine).map_err(|error| error.to_string())?;
    if node.is_here(&machine) {
        node.works_here(acting)?;
        let path = location.path.clone();
        return tokio::task::spawn_blocking(move || heads(Path::new(&path)))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string());
    }
    let call = RemoteCall::Canon { path: location.path.clone() };
    match node.call(acting, &machine, Purpose::Hands, call).await {
        Ok(Ok(RemoteReply::Canon { heads })) => Ok(heads),
        Ok(Ok(_)) => Err(format!("{} answered something else", location.machine)),
        Ok(Err(failure)) => Err(failure.message),
        Err(CallFailure::Offline { name, .. }) => Err(format!("{name} is offline")),
        Err(failure) => Err(failure.to_string()),
    }
}

/// Each Markdown document directly in `directory`, by name: its title, the
/// first heading, and the paragraph after it, which says what it covers.
pub fn heads(directory: &Path) -> io::Result<Vec<CanonHead>> {
    let directory = fastctx_platform::local_path(directory)?;
    let mut files: Vec<_> = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.is_file() && path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    files.sort();
    let mut heads = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)?;
        let file = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        heads.push(head(file, &text));
    }
    Ok(heads)
}

fn head(file: String, text: &str) -> CanonHead {
    let mut lines = text.lines().map(str::trim);
    let title = lines.by_ref().find_map(|line| line.strip_prefix("# ")).map(str::trim).unwrap_or_default();
    // The paragraph after the title: its lines up to the first blank one,
    // skipping blank lines before it.
    let scope: Vec<&str> = lines.skip_while(|line| line.is_empty()).take_while(|line| !line.is_empty()).collect();
    CanonHead { title: if title.is_empty() { file.clone() } else { title.to_string() }, file, scope: scope.join(" ") }
}
