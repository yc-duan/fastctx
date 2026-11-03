//! The workbench by login (`world-layers.md`, 工作台：在一个登录上打开): the
//! Codex and Claude Code logins of this machine it is open in, kept with
//! the logins (`kept::KeptAgent::workbench`) and in memory, since the
//! node's gates ask at every turn. With it open in a login the World starts
//! and drives conversations there, reads its limits and asks its models;
//! with it closed, the login is only connected.
//!
//! It was kept by account before (`crate::layers`). The logins it was open
//! in are worked out once (`seed`): those this node holds conversations of
//! Fastctx's own in, for an account on the workbench, so that none of them
//! stops; until every such account's layer can be read, the gates answer
//! by account as before.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;
use fastctx_types::layers::Layer;
use fastctx_types::residents::About;

use super::{Context, kept};
use crate::Node;

/// Said when a conversation would be started or driven in a login the
/// workbench is not open in.
pub(crate) const CLOSED: &str =
    "the workbench is not open in this login; conversations in Fastctx need it, opened on the agents page";

/// What the node knows of the workbench while it runs.
#[derive(Default)]
pub(crate) struct Open {
    /// Read from the kept record: until then, nothing.
    read: bool,
    /// Whether the logins it was open in by account were worked out.
    seeded: bool,
    /// The logins it is open in: their agent and directory.
    dirs: Vec<(String, PathBuf)>,
    /// Each agent's default login, as the user's environment names it.
    defaults: Vec<(String, PathBuf)>,
}

/// Whether the workbench is open in `agent`'s login `home`, its default one
/// where none; before the logins it was open in are worked out, whether the
/// account `acting` works for was on it.
pub(crate) fn is_open(node: &Node, acting: &Acting, agent: &str, home: Option<&Path>) -> bool {
    if !matches!(agent, "codex" | "claude") {
        return false;
    }
    let state = state(node);
    if !state.seeded {
        drop(state);
        return crate::layers::of(node, acting) == Layer::Workbench;
    }
    let Some(dir) =
        home.or_else(|| state.defaults.iter().find(|(name, _)| name == agent).map(|(_, dir)| dir.as_path()))
    else {
        return false;
    };
    state.dirs.iter().any(|(name, open)| name == agent && crate::homes::same_dir(open, dir))
}

/// Whether the workbench is open in `agent`'s login `dir`, once the logins it
/// was open in by account were worked out; before that, nothing is said, and
/// a change leaves the login holding what it holds.
pub(crate) fn known(node: &Node, agent: &str, dir: &Path) -> Option<bool> {
    if !matches!(agent, "codex" | "claude") {
        return None;
    }
    let state = state(node);
    state.seeded.then(|| state.dirs.iter().any(|(name, open)| name == agent && crate::homes::same_dir(open, dir)))
}

/// Refuses to start a conversation in a login the workbench is not open in.
pub(crate) fn may_start(node: &Node, acting: &Acting, agent: &str, home: Option<&Path>) -> Result<(), Failure> {
    if is_open(node, acting, agent, home) { Ok(()) } else { Err(Failure::refused(CLOSED)) }
}

/// Whether the World may start a turn in the conversation `about` now: one
/// it holds waits while the workbench is closed in its login.
pub(crate) fn drives(node: &Node, about: &About) -> bool {
    !about.world_managed() || is_open(node, &about.acting, about.host.agent.as_str(), about.home.as_deref())
}

/// Opens or closes the workbench in `agent`'s login `dir`, once a change
/// of its files made it so (`super::change`).
pub(crate) fn set(node: &Node, agent: &str, dir: &Path, open: bool) -> std::io::Result<()> {
    kept::change(node, |record| {
        let dirs = &mut record.of_mut(agent).workbench;
        dirs.retain(|known| !crate::homes::same_dir(known, dir));
        if open {
            dirs.push(dir.to_path_buf());
        }
    })?;
    let mut state = state(node);
    state.dirs.retain(|(name, known)| !(name == agent && crate::homes::same_dir(known, dir)));
    if open {
        state.dirs.push((agent.to_owned(), dir.to_path_buf()));
    }
    Ok(())
}

/// Closes the workbench in each login Fastctx is no longer connected to
/// (`super::refresh`): taken out by the command line, or by hand.
pub(super) fn prune(node: &Node, connected: &[(&str, PathBuf)]) {
    let stale: Vec<(String, PathBuf)> = {
        let state = state(node);
        if !state.seeded {
            return;
        }
        state
            .dirs
            .iter()
            .filter(|(agent, dir)| {
                !connected.iter().any(|(name, home)| name == agent && crate::homes::same_dir(home, dir))
            })
            .cloned()
            .collect()
    };
    for (agent, dir) in stale {
        match set(node, &agent, &dir, false) {
            Ok(()) => node.log.line(format_args!(
                "the workbench is closed in the {agent} login {}: Fastctx is no longer connected there",
                dir.display()
            )),
            Err(error) => node.log.line(format_args!("keeping where the workbench is open: {error}")),
        }
    }
}

/// The workbench was opened or closed in a login: what waited for it there
/// is tried at once, and the login asked again, its models too.
pub(crate) fn changed(node: &Arc<Node>) {
    node.shown_changed();
    for held in node.residents.everyone().filter(|held| held.about.world_managed()) {
        crate::lifecycle::retry::changed(node, &held.id);
    }
}

/// The record's view as the node holds it, read on first use.
fn state(node: &Node) -> std::sync::MutexGuard<'_, Open> {
    let mut state = node.logins.workbench.lock().expect("never poisoned");
    if !state.read {
        match kept::read(node) {
            Ok(record) => {
                state.seeded = record.workbench_seeded;
                state.dirs = ["codex", "claude"]
                    .into_iter()
                    .flat_map(|agent| record.of(agent).workbench.iter().map(move |dir| (agent.to_owned(), dir.clone())))
                    .collect();
                state.read = true;
            }
            Err(error) => node.log.line(format_args!("reading where the workbench is open: {error}")),
        }
    }
    state
}

/// Each agent's default login as the user's environment names it now, for
/// a conversation that records none (`About::home`).
pub(super) fn defaults(node: &Node, context: &Context) {
    let mut state = node.logins.workbench.lock().expect("never poisoned");
    state.defaults = ["codex", "claude"]
        .into_iter()
        .map(|agent| (agent.to_owned(), context.default_of(agent).to_path_buf()))
        .collect();
}

/// Works out once the logins the workbench was open in while it was kept by
/// account: each one this node holds a conversation of Fastctx's own in,
/// for an account on the workbench. Nothing yet while some such account's
/// layer cannot be read.
pub(super) fn seed(node: &Node, context: &Context) {
    defaults(node, context);
    if state(node).seeded {
        return;
    }
    let mut open: Vec<(String, PathBuf)> = Vec::new();
    for held in node.residents.everyone().filter(|held| held.removed.is_none() && held.about.world_managed()) {
        let agent = held.about.host.agent.as_str();
        if !matches!(agent, "codex" | "claude") {
            continue;
        }
        match crate::layers::readable(node, &held.about.acting) {
            None => return,
            Some(Layer::Connect) => continue,
            Some(Layer::Workbench) => {}
        }
        let dir = held.about.home.clone().unwrap_or_else(|| context.default_of(agent).to_path_buf());
        if !open.iter().any(|(name, known)| name == agent && crate::homes::same_dir(known, &dir)) {
            open.push((agent.to_owned(), dir));
        }
    }
    let kept = kept::change(node, |record| {
        for (agent, dir) in &open {
            let dirs = &mut record.of_mut(agent).workbench;
            if !dirs.iter().any(|known| crate::homes::same_dir(known, dir)) {
                dirs.push(dir.clone());
            }
        }
        record.workbench_seeded = true;
    });
    match kept {
        Ok(()) => {
            let mut state = state(node);
            for (agent, dir) in open {
                if !state.dirs.iter().any(|(name, known)| *name == agent && crate::homes::same_dir(known, &dir)) {
                    node.log.line(format_args!("the workbench stays open in the {agent} login {}", dir.display()));
                    state.dirs.push((agent, dir));
                }
            }
            state.seeded = true;
        }
        Err(error) => node.log.line(format_args!("keeping where the workbench is open: {error}")),
    }
}
