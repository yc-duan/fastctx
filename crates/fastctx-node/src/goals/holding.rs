//! A user stop commits its intent, local hold and frozen input revision together
//! before the host abort. Projection repair never changes that authority.
use super::{intents, lifecycle, local, operations, projection, state, words};
use crate::{Node, records};
use fastctx_types::{
    ResidentId,
    goals::{Author, Status, Stop, StopKind, View},
    ipc::Failure,
};
use std::{io, path::Path, sync::Arc};
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
async fn work<T: Send + 'static>(action: impl FnOnce() -> io::Result<T> + Send + 'static) -> Result<T, Failure> {
    fastctx_platform::task::spawn_blocking(action).await.map_err(failure)?.map_err(failure)
}
fn prepare(
    node: &Node,
    id: &ResidentId,
) -> io::Result<(fastctx_types::layout::ConversationDir, bool, Option<local::PreparedBridge>)> {
    let directory = node.world.conversation(id);
    local::prepare(&directory)?;
    crate::inbox::presence::prepare(&directory)?;
    let claude = node.residents.about(id).is_some_and(|(about, _)| about.host.agent.as_str() == "claude");
    let fallback = if claude { local::fallback(&directory, projection::exchange(node, id).as_deref())? } else { None };
    Ok((directory, claude, fallback))
}
/// Recheck the stop hold in the same transaction that accepts the local view.
/// The caller installs context between passes and repeats if a stop intervened.
pub(super) async fn apply(
    node: &Arc<Node>,
    id: &ResidentId,
    view: &View,
    installed: Option<&str>,
) -> Result<(String, Option<View>), Failure> {
    let enabled = crate::account_features::read(node, &view.account).await?.goals;
    let (working, resident, view, installed) = (node.clone(), id.clone(), view.clone(), installed.map(str::to_string));
    let (text, accepted, published) = work(move || {
        let (directory, claude, fallback) = prepare(&working, &resident)?;
        let (committed, shown) = (working.clone(), resident.clone());
        records::write_then_at(
            &directory,
            fastctx_storage::Priority::Interactive,
            serde_json::to_vec(&view)?.len(),
            move |rows| {
                // A view read before the one kept here is not taken: what the
                // account decided since stays, and is applied again.
                let view = match rows.get::<local::Local>(local::LOCAL, "current")?.map(|local| local.view) {
                    Some(kept) if local::newer(&kept, &view) => kept,
                    _ => view,
                };
                let held = local::hold(rows)?;
                let holds = held.as_ref().is_some_and(|held| !held.superseded(&view));
                let text = if !enabled || holds || working.residents.removed(&resident).is_some() {
                    String::new()
                } else {
                    view.goal.as_ref().map(words::context).unwrap_or_default()
                };
                if held.is_some() && !holds {
                    local::clear_hold(rows)?;
                }
                let mut view = view;
                view.pending = lifecycle::pending(&working, &resident, &view, held.as_ref());
                let accepted = installed.as_ref() == Some(&text);
                let published = accepted
                    || view.pending.is_some()
                    || view.goal.as_ref().is_some_and(|goal| goal.status == Status::Paused);
                if published {
                    local::install(rows, &view, claude, false, fallback.as_ref())?;
                } else if let Some(fallback) = &fallback {
                    fallback.finish(rows)?;
                }
                Ok((text, accepted.then_some(view), published))
            },
            move |(_, accepted, published)| {
                if let Some(view) = accepted {
                    operations::publish(&committed, &shown, view);
                }
                if *published {
                    committed.goals.projection_changed.notify_one();
                }
            },
        )
    })
    .await?;
    if published {
        projection::repair(node, id, projection::exchange(node, id)).await.map_err(failure)?;
    }
    Ok((text, accepted))
}
/// Install the latest projection before a newly identified host becomes ready.
pub(crate) async fn registered(node: &Arc<Node>, id: &ResidentId, exchange: &Path) -> Result<(), Failure> {
    let (working, resident, path) = (node.clone(), id.clone(), exchange.to_path_buf());
    work(move || {
        let directory = working.world.conversation(&resident);
        local::prepare(&directory)?;
        crate::inbox::presence::prepare(&directory)?;
        let fallback = local::fallback(&directory, Some(&path))?;
        records::write_then_at(
            &directory,
            fastctx_storage::Priority::Interactive,
            1024,
            move |rows| local::registered(rows, fallback.as_ref()),
            move |_| working.goals.projection_changed.notify_one(),
        )
    })
    .await?;
    projection::repair(node, id, Some(exchange.to_path_buf())).await.map_err(failure)
}
pub(crate) async fn stop(node: &Arc<Node>, id: &ResidentId, turn: Option<String>) -> Result<(), Failure> {
    let (working, resident) = (node.clone(), id.clone());
    let runtime = tokio::runtime::Handle::current();
    let result = work(move || {
        let (directory, claude, fallback) = prepare(&working, &resident)?;
        intents::prepare(&directory)?;
        let cached = working.goals.views.lock().expect("goal views").get(&resident).cloned();
        let (committed, shown) = (working.clone(), resident.clone());
        records::write_then_at(
            &directory,
            fastctx_storage::Priority::Control,
            4096,
            move |rows| {
                let stopped = (|| {
                    let Some(mut view) =
                        rows.get::<local::Local>(local::LOCAL, "current")?.map(|local| local.view).or(cached)
                    else {
                        return Ok(None);
                    };
                    let Some(mut goal) =
                        view.goal.clone().filter(|goal| goal.retired.is_none() && goal.status != Status::Complete)
                    else {
                        return Ok(None);
                    };
                    let operation = crate::speech::new_id("goal-stop").to_string();
                    let at = jiff::Timestamp::now();
                    let reason = "The user stopped this turn; the goal is paused.".to_string();
                    goal.status = Status::Paused;
                    goal.stop = Some(Stop { kind: StopKind::User, reason: reason.clone(), resume_at: None });
                    goal.claim = None;
                    goal.runtime.pending = None;
                    goal.runtime.retry = None;
                    state::remember(
                        &mut goal,
                        state::Change { operation: &operation, author: Author::World, turn, at },
                        "user_stopped",
                        reason.clone(),
                    );
                    let intent = intents::Intent {
                        access: None,
                        operation: operation.clone(),
                        input_hash: operation.clone(),
                        resident: goal.resident.clone(),
                        account: view.account.clone(),
                        expected: view.version,
                        proposed: Some(goal.clone()),
                        at,
                        phase: intents::Phase::Prepared,
                    };
                    let held = lifecycle::Hold {
                        base: view.version,
                        kind: lifecycle::HoldKind::UserStop,
                        at,
                        goal: goal.id,
                        operation,
                        reason,
                    };
                    intents::keep_in(rows, &intent)?;
                    local::put_hold(rows, &held)?;
                    view.pending = Some(held.pending());
                    local::install(rows, &view, claude, true, fallback.as_ref())?;
                    Ok(Some((view, intent.resident)))
                })();
                if let Some(fallback) = &fallback {
                    fallback.finish(rows)?;
                }
                stopped
            },
            move |result| {
                if let Some((view, resident)) = result {
                    operations::publish(&committed, &shown, view);
                    committed.goals.projection_changed.notify_one();
                    let (node, resident) = (committed.clone(), resident.clone());
                    fastctx_platform::task::spawn_on(&runtime, async move {
                        super::recovery::schedule(&node, resident);
                    });
                }
            },
        )
    })
    .await?;
    if result.is_some() {
        projection::repair(node, id, projection::exchange(node, id)).await.map_err(failure)?;
    }
    Ok(())
}
