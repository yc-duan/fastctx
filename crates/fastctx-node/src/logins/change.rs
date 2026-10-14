//! Putting the World's tools into a login of this machine, which lists it,
//! and taking them out, which removes it from the list: done by this
//! machine's node, with the node's own view of the user's environment, so
//! the login changed is the login the record shows (`world-agent-logins.md`,
//! 接入). Each is previewed first, with nothing written, and runs step by
//! step, every step kept in the record as it goes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_install::apply::{self, Places};
use fastctx_install::codex::Approval;
use fastctx_types::AgentName;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{ChangeEnd, ChangePreview, ChangeStep, Kept, LoginChange, OutputLimit, StepState};
use jiff::Timestamp;

use super::Context;
use crate::Node;

/// The places of an apply of the login `dir`, as this node sees them.
fn places(node: &Node, agent: &AgentName, dir: &Path) -> Result<(Places, apply::Host), Failure> {
    let host = super::host(agent.as_str())?;
    let context = Context::of(node)?;
    let places = context.places.at_home(host, dir).map_err(|error| Failure::new(error.to_string()))?;
    Ok((places, host))
}

/// What putting the World's tools into the login (`into`), taking back what
/// the user changed of them with `take_back`, or taking them out, would
/// change.
pub(crate) async fn preview(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    take_back: bool,
) -> Result<ChangePreview, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        super::known(&node, &agent, &dir)?;
        let (places, host) = places(&node, &agent, &dir)?;
        let mut preview =
            apply::preview(&places, host, into, edits(take_back)).map_err(|error| Failure::new(error.to_string()))?;
        preview.dir = dir;
        Ok(preview)
    })
    .await
    .map_err(|error| Failure::new(format!("previewing the change: {error}")))?
}

/// Puts the World's tools into the login, or takes them out, telling the
/// record each step; answered with the change once it ended.
pub(crate) async fn change(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
    take_back: bool,
) -> Result<LoginChange, Failure> {
    let _changing = node.logins.changing.lock().await;
    {
        let node = node.clone();
        let (agent, dir) = (agent.clone(), dir.clone());
        tokio::task::spawn_blocking(move || super::known(&node, &agent, &dir))
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
    }
    let started = LoginChange { into, started_at: Timestamp::now(), steps: Vec::new(), end: None };
    set(node, &agent, &dir, |change| *change = started.clone());
    let progress: apply::Progress = {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.clone());
        Arc::new(move |step: ChangeStep| set(&node, &agent, &dir, |change| told(&mut change.steps, step.clone())))
    };
    let ran = {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            let (places, host) = places(&node, &agent, &dir)?;
            let approval = if raise_output_limit { Approval::RaiseOutputLimit } else { Approval::Require };
            let done = if into {
                apply::apply_reporting(&places, host.only(), approval, edits(take_back), Some(progress))
            } else {
                apply::unapply_reporting(&places, host.only(), Some(progress))
            };
            Ok::<_, Failure>(done)
        })
        .await
        .map_err(|error| Failure::new(format!("changing the login: {error}")))?
    };
    let at = Timestamp::now();
    let end = match ran {
        Ok(Ok(report)) => {
            let word = if into { "put the World's tools into" } else { "took the World's tools out of" };
            node.log.line(format_args!("the user {word} the {agent} login {}", dir.display()));
            super::refresh::settled(node, &agent, &dir);
            ChangeEnd::Done {
                at,
                kept: report
                    .notices
                    .into_iter()
                    .map(|notice| Kept { location: notice.location, reason: notice.reason })
                    .collect(),
            }
        }
        Ok(Err(apply::Error::Edit(fastctx_install::Error::TokenLimitApproval { location, current, requested }))) => {
            ChangeEnd::Failed {
                at,
                reason: format!(
                    "{location} is {current}, and the World's answers need {requested}; it is the limit of every tool in Codex, so it is raised only when the user says so"
                ),
                output_limit: Some(OutputLimit { location, current, requested }),
            }
        }
        Ok(Err(error)) => ChangeEnd::Failed { at, reason: error.to_string(), output_limit: None },
        Err(failure) => ChangeEnd::Failed { at, reason: failure.message, output_limit: None },
    };
    let mut ended = None;
    set(node, &agent, &dir, |change| {
        if let ChangeEnd::Failed { reason, .. } = &end {
            for step in &mut change.steps {
                if step.state == StepState::Running {
                    step.state = StepState::Failed { reason: reason.clone() };
                }
            }
        }
        change.end = Some(end.clone());
        ended = Some(change.clone());
    });
    Ok(ended.expect("the change was set"))
}

fn edits(take_back: bool) -> apply::Edits {
    if take_back { apply::Edits::TakeBack } else { apply::Edits::Keep }
}

/// Changes the record of the change running in a login.
fn set(node: &Node, agent: &AgentName, dir: &Path, edit: impl FnOnce(&mut LoginChange)) {
    let mut changes = node.logins.changes.lock().expect("never poisoned");
    let index = changes.iter().position(|(owner, path, _)| owner == agent && crate::homes::same_dir(path, dir));
    let index = match index {
        Some(index) => index,
        None => {
            let blank = LoginChange { into: true, started_at: Timestamp::now(), steps: Vec::new(), end: None };
            changes.push((agent.clone(), dir.to_path_buf(), blank));
            changes.len() - 1
        }
    };
    edit(&mut changes[index].2);
}

/// A step told: the same step told again while running replaces its state.
fn told(steps: &mut Vec<ChangeStep>, step: ChangeStep) {
    if let Some(last) = steps.iter_mut().rev().find(|known| same_step(&known.step, &step.step))
        && last.state == StepState::Running
    {
        *last = step;
        return;
    }
    steps.push(step);
}

fn same_step(a: &fastctx_types::logins::Step, b: &fastctx_types::logins::Step) -> bool {
    use fastctx_types::logins::Step;
    match (a, b) {
        (Step::Program, Step::Program) | (Step::Files { .. }, Step::Files { .. }) => true,
        (Step::Command { command: a }, Step::Command { command: b }) => a == b,
        (Step::Check { scope: a }, Step::Check { scope: b }) => a == b,
        _ => false,
    }
}
