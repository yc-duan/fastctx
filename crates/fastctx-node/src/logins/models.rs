//! The models each Claude Code login here offers (`world-gui.md`, 对话): the
//! World's own table, by the families the login's newest session saw it
//! offer, as those who hold the machine changed it. The agents' pages, a
//! conversation's model panel and the model the World starts a conversation
//! on all read it here, so everyone who uses the login, those it assists
//! too, is offered the same models, and the World uses no other.

use std::path::{Path, PathBuf};

use fastctx_hosts::claude::models as table;
use fastctx_types::AgentName;
use fastctx_types::agents::{AgentHome, HomeFound};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::models::{ModelChange, ModelEdits};
use jiff::Timestamp;

use super::kept::{self, Kept, KeptModels};
use crate::Node;

/// What those who hold the machine changed in the models of the login `dir`.
pub(super) fn edits(kept: &Kept, dir: &Path) -> ModelEdits {
    kept.claude
        .models
        .iter()
        .find(|kept| crate::homes::same_dir(&kept.dir, dir))
        .map(|kept| kept.edits.clone())
        .unwrap_or_default()
}

/// The aliases the newest session of the login `dir` saw it offer, among
/// `readings` (`exchange::aliases`); none before a session ran there.
pub(super) fn aliases(
    readings: &[(PathBuf, Timestamp, Vec<String>)],
    homes: &[AgentHome],
    dir: &Path,
) -> Option<Vec<String>> {
    readings
        .iter()
        .find(|(transcript, _, _)| {
            super::owner(homes, transcript).is_some_and(|home| crate::homes::same_dir(home, dir))
        })
        .map(|(_, _, aliases)| aliases.clone())
}

/// What a login offers: the aliases its Claude Code was last seen to offer,
/// and what those who hold it changed.
pub(crate) struct Offering {
    pub aliases: Option<Vec<String>>,
    pub edits: ModelEdits,
}

impl Offering {
    /// The login a conversation's `home` names here, `None` being the
    /// default one.
    pub fn of(node: &Node, home: Option<&Path>) -> Result<Self, Failure> {
        let context = super::Context::of(node)?;
        let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
        let dir = home.unwrap_or(&context.claude_default);
        let homes: Vec<AgentHome> = super::dirs(node, &context, &kept, "claude")?
            .into_iter()
            .map(|(dir, _)| AgentHome { dir, found: HomeFound::Added })
            .collect();
        let readings = fastctx_hosts::claude::exchange::aliases(node.world.path())
            .map_err(|error| Failure::new(format!("reading the models Claude Code offered: {error}")))?;
        Ok(Self { aliases: aliases(&readings, &homes, dir), edits: edits(&kept, dir) })
    }

    pub fn ids(&self) -> Vec<String> {
        table::offered(self.aliases.as_deref(), &self.edits).iter().map(|model| model.id().to_owned()).collect()
    }

    /// Refuses a model the login does not offer, naming those it does.
    pub fn require(&self, model: &str) -> Result<(), Failure> {
        if table::offers(self.aliases.as_deref(), &self.edits, model) {
            return Ok(());
        }
        Err(Failure::not_run(format!(
            "this Claude Code login does not offer {model}; it offers {}",
            self.ids().join(", ")
        )))
    }

    /// The model a conversation starts on when none was named: `preferred`
    /// (the account's own choice) when the login offers it, else its first.
    pub fn starting(&self, preferred: Option<&str>) -> Option<String> {
        let ids = self.ids();
        preferred
            .filter(|preferred| ids.iter().any(|id| id == preferred))
            .map(str::to_owned)
            .or_else(|| ids.into_iter().next())
    }
}

/// What those who hold the machine changed in the models of the login a
/// conversation's `home` names here.
pub(crate) fn edits_of(node: &Node, home: Option<&Path>) -> Result<ModelEdits, Failure> {
    let context = super::Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    Ok(edits(&kept, home.unwrap_or(&context.claude_default)))
}

/// Changes the models the Claude Code login `dir` here offers. A change
/// that would leave it none is refused.
pub(crate) fn change(node: &Node, agent: &AgentName, dir: &Path, change: &ModelChange) -> Result<(), Failure> {
    if agent.as_str() != "claude" {
        return Err(Failure::not_run("only Claude Code's models are the World's to list; Codex lists its own"));
    }
    super::known(node, agent, dir)?;
    let aliases = Offering::of(node, Some(dir))?.aliases;
    let dir = dir.to_path_buf();
    kept::change(node, move |kept| {
        let mut changed = edits(kept, &dir);
        changed.apply(change, |id| table::find(id).is_some()).map_err(Failure::not_run)?;
        if table::offered(aliases.as_deref(), &changed).is_empty() {
            return Err(Failure::not_run("a login offers at least one model; add another before taking this one out"));
        }
        kept.claude.models.retain(|kept| !crate::homes::same_dir(&kept.dir, &dir));
        if !changed.is_empty() {
            kept.claude.models.push(KeptModels { dir, edits: changed });
        }
        Ok(())
    })
    .map_err(|error| Failure::new(format!("keeping the login's models: {error}")))?
}
