//! `fastctx w`: who is in this town and what they are doing (`specs/world-town.md`).

use super::who::{self, Query};
use super::w::{self, View};
use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use std::process::ExitCode;

/// The resident on whose behalf a command runs; the World's `run` sets it.
const RESIDENT_VARIABLE: &str = super::session::RESIDENT_VARIABLE;
/// The town that resident is in, when it is in one; the World's `run` sets it.
pub(crate) const TOWN_VARIABLE: &str = "FASTCTX_TOWN";

#[derive(Debug, clap::Args)]
pub(crate) struct WArgs {
    /// Also list residents that are asleep or have left: earlier colleagues you can wake.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// A town to look at instead of your own, or resident and job ids to see in detail.
    #[arg(value_name = "TOWN | ID")]
    targets: Vec<String>,
}

pub(crate) fn run(args: WArgs) -> Result<ExitCode, String> {
    let paths = WorldPaths::from_control(&ControlPaths::discover()?);
    let config = crate::world::load_config(&paths)?.ok_or_else(|| {
        "This machine is not in a World; fastctx w looks at the residents of a World's towns.".to_string()
    })?;
    let caller = std::env::var(RESIDENT_VARIABLE).ok().filter(|id| !id.is_empty());
    let (ids, towns): (Vec<String>, Vec<String>) = args.targets.into_iter().partition(|target| is_id(target));
    let query = match (ids.is_empty(), towns.as_slice()) {
        (false, []) => Query { ids, ..Query::default() },
        (true, []) => Query { town: own_town(&paths.dir, &config.name, caller.as_deref()), all: args.all, ids: Vec::new() },
        (true, [town]) => Query { town: Some(town.clone()), all: args.all, ids: Vec::new() },
        _ => return Err("fastctx w takes one town, or one or more resident and job ids.".to_string()),
    };
    let answers = vec![who::answer(&paths.dir, &config.name, &query)];
    let view = View { caller: caller.as_deref(), query: &query, answers: &answers, silent: &[] };
    print!("{}", w::render(&view, time::OffsetDateTime::now_utc()));
    Ok(ExitCode::SUCCESS)
}

/// Resident ids look like `codex-7f3a`, job ids like `j-k3x9a2`; town names are free words.
fn is_id(target: &str) -> bool {
    super::records::is_resident_id(target) || (target.starts_with("j-") && target.len() > 2)
}

/// The caller's town: as `run` passed it, or from its records when it lives on this machine.
fn own_town(world_dir: &std::path::Path, machine: &str, caller: Option<&str>) -> Option<String> {
    if let Ok(town) = std::env::var(TOWN_VARIABLE) {
        return Some(town).filter(|town| !town.is_empty());
    }
    let caller = caller?;
    let found = who::answer(world_dir, machine, &Query { ids: vec![caller.to_string()], ..Query::default() });
    found.residents.into_iter().next().and_then(|facts| facts.about.town)
}
