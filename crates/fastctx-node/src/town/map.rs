//! A town's map (`world-map.md`): made from its record, its journals and
//! its canon each time it is asked for, never kept.

use std::io;
use std::path::Path;
use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_types::TownName;
use fastctx_types::ipc::Failure;
use fastctx_types::project::{CanonHead, Location, MapView, Stamp};
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;
use crate::town::read;

/// The map of `town`, from this machine's copy of its ledger.
pub async fn view(node: &Arc<Node>, town: &TownName) -> Result<MapView, Failure> {
    let (world, name) = (node.world.clone(), town.clone());
    let (records, journals) = tokio::task::spawn_blocking(move || {
        let records = read::records(&world, &name)?;
        let journals = read::summaries(&world, &name)?;
        io::Result::Ok((records, journals))
    })
    .await
    .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?
    .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?;
    let (Some(first), Some(record)) = (records.first(), records.last()) else {
        return Err(no_ledger(node, town));
    };
    let opened = Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
    let canon = match &record.canon {
        Some(location) => Some(canon(node, location).await),
        None => None,
    };
    Ok(MapView {
        town: town.clone(),
        ledger: crate::world::native(&node.world.town(town).ledger()),
        record: record.clone(),
        opened,
        journals,
        canon,
    })
}

/// What a resident living in `town` is told of it: its map, and how the
/// town's journals are kept.
pub async fn brief(node: &Arc<Node>, town: &TownName) -> Result<String, Failure> {
    Ok(fastctx_hands::town::brief(&view(node, town).await?))
}

/// Why there is no map of a town here.
pub fn no_ledger(node: &Node, town: &TownName) -> Failure {
    Failure::new(format!(
        "{} keeps no ledger of a town {town}: there is no such town, or it does not span this machine",
        node.machine()
    ))
}

/// The documents of a canon, read where it is.
async fn canon(node: &Arc<Node>, location: &Location) -> Result<Vec<CanonHead>, String> {
    if location.machine == node.machine() {
        let path = location.path.clone();
        return tokio::task::spawn_blocking(move || heads(Path::new(&path)))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string());
    }
    let Some(link) = node.link() else {
        return Err(format!("{} is in no World with {}", node.machine(), location.machine));
    };
    match link.call(&location.machine, RemoteCall::Canon { path: location.path.clone() }).await {
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
