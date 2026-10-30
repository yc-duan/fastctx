//! The goal engine's seams onto sentinels (`world-goal.md`, 目标的哨兵): the
//! goal's own sentinel judges a stop, the sentinels a conversation left are
//! what it waits on, and a goal that ends takes its sentinel with it.
use super::goal;
use crate::Node;
use crate::goals::records::Reply;
use crate::goals::reviewer::{Answer, Materials, Readiness, Reviewer};
use crate::turn_evidence::waiting::SentinelWaits;
use fastctx_types::{
    Address, ResidentId,
    access::HubAccount,
    goals::runtime::{ReviewCause, Verdict},
    ipc::Failure,
    sentinel::{FullMaterial, GoalReview, GoalTrigger, Judgment},
    sentinel_settings::HomeStatus,
    turn_evidence::{Liveness, StopReason, WaitHandle, WaitKind},
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};

fn node(weak: &Weak<Node>) -> Result<Arc<Node>, Failure> {
    weak.upgrade().ok_or_else(|| Failure::new("the node has gone"))
}

/// Judges a goal's stops with the goal's sentinel at the account's home.
pub(crate) struct GoalSentinel(pub Weak<Node>);

impl Reviewer for GoalSentinel {
    fn readiness<'a>(&'a self, account: &'a HubAccount) -> Reply<'a, Readiness> {
        Box::pin(async move {
            let node = node(&self.0)?;
            let link = node
                .link_of(&account.hub)
                .ok_or_else(|| Failure::not_run("the goal account's hub is not connected"))?;
            let (_, settings) = fastctx_sentinel::settings::read(node.sentinels.records()?, account)
                .await
                .map_err(|e| Failure::not_run(e.to_string()))?;
            let (Some(home), Some(_)) = (settings.home.as_ref(), settings.model.as_ref()) else {
                return Ok(Readiness::Absent { reason: words::NOT_SET_UP.into() });
            };
            match super::settings::home_status(&link, &account.account, Some(home)).await {
                HomeStatus::Ready => {}
                HomeStatus::Offline => return Ok(Readiness::Offline { reason: words::home_offline(home) }),
                HomeStatus::Unavailable { reason } => return Err(Failure::not_run(reason)),
                HomeStatus::Unset | HomeStatus::Forbidden | HomeStatus::TooOld => {
                    return Ok(Readiness::Absent { reason: words::home_unusable(home) });
                }
            }
            let credentials =
                node.sentinels.credentials.get().ok_or_else(|| Failure::not_run("sentinel keys are not connected"))?;
            let keys = credentials.status(account, home).await.map_err(Failure::not_run)?;
            Ok(if keys.model { Readiness::Ready } else { Readiness::Absent { reason: words::NO_KEY.into() } })
        })
    }

    fn review<'a>(&'a self, request: &'a str, materials: &'a Materials) -> Reply<'a, Answer> {
        Box::pin(async move {
            let node = node(&self.0)?;
            let (registration, away) =
                match goal::open(&node, &materials.account, &materials.conversation, &materials.objective).await {
                    Ok(opened) => opened,
                    Err(error) => return Ok(Answer::Failed { reason: error.message }),
                };
            if let Some(reason) = away {
                return Ok(Answer::Offline { reason });
            }
            // Its home was ready a moment ago, so a judgment that does not come
            // back is a failed attempt, not a wait.
            Ok(match goal::judge(&node, &registration, request, review_of(materials)).await {
                Judgment::Sound { reason } => Answer::Judged(Verdict::Sound { reason }),
                Judgment::Advice { looked, saw, thinks } => Answer::Judged(Verdict::Advice { looked, saw, thinks }),
                Judgment::NotJudged { reason } | Judgment::Unavailable { reason } => Answer::Failed { reason },
            })
        })
    }
}

/// The material the engine gathered, in the shape the goal's sentinel reads.
fn review_of(materials: &Materials) -> GoalReview {
    let full = materials
        .full_path
        .clone()
        .or_else(|| materials.world_actions.as_ref().map(|evidence| evidence.full_path.clone()))
        .map(|path| FullMaterial {
            machine: materials.full_machine.clone().unwrap_or_else(|| materials.conversation.machine.clone()),
            path,
        });
    GoalReview {
        trigger: match materials.cause {
            ReviewCause::Idle => GoalTrigger::Idle,
            ReviewCause::Blocked => GoalTrigger::Blocked,
            ReviewCause::Paused => GoalTrigger::Paused,
        },
        suggestions_left: materials.suggestions_left,
        goal: materials.objective.clone(),
        plan_claim: materials.plan_claim.clone(),
        actions: materials.world_actions.as_ref().map(|evidence| evidence.summary.clone()),
        turn_end: materials.stopped.as_ref().map(stop_said),
        last_words_claim: materials.last_words_claim.clone(),
        blocked_claim: materials.blocked_claim.clone(),
        user_words: materials.owner_words.iter().map(|words| format!("[{}] {}", words.at, words.text)).collect(),
        previous_judgments: materials.previous_judgments.clone(),
        machines: materials.machines.clone(),
        now: materials.now,
        full,
    }
}

/// How the turn ended, as its kind and the host's own words.
fn stop_said(stop: &StopReason) -> String {
    let kind =
        serde_json::to_value(stop.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default();
    match &stop.message {
        Some(message) => format!("{kind}: {message}"),
        None => kind,
    }
}

/// The sentinels a conversation left, which will wake it when they hold.
pub(crate) struct Waits(pub Weak<Node>);

impl SentinelWaits for Waits {
    fn waiting<'a>(
        &'a self,
        owner: &'a ResidentId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<WaitHandle>, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let node = node(&self.0)?;
            let address = Address { machine: node.machine(), resident: owner.clone() };
            let registrations = goal::waiting(&node, &address).await?;
            Ok(registrations
                .into_iter()
                .map(|registration| {
                    let home = registration.definition.home.clone();
                    let view = node
                        .link_of(&registration.scope.hub)
                        .and_then(|link| crate::authorization::current_view(&link).ok());
                    let liveness = match view {
                        Some(view) if view.is_me(&home) || view.member(&home).is_some_and(|member| member.online) => {
                            Liveness::Alive
                        }
                        Some(_) => Liveness::Unknown { reason: words::home_offline(&home) },
                        None => Liveness::Unknown { reason: "the sentinel's hub is not connected".into() },
                    };
                    WaitHandle {
                        id: registration.definition.id.to_string(),
                        kind: WaitKind::Sentinel,
                        machine: home,
                        since: None,
                        liveness,
                        // Its report is kept for the conversation even while its home is away.
                        will_wake: true,
                        completion: None,
                        summary: registration.definition.condition.clone(),
                        output: None,
                        output_growth: None,
                    }
                })
                .collect())
        })
    }
}

/// Ends the sentinel of a goal that was cleared, completed or retired, in
/// the background: a goal's sentinel lives only as long as its goal. Ending
/// one already gone does nothing, so it is tried again until it is done.
pub(crate) fn retire(node: &Arc<Node>, scope: HubAccount, owner: Address) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut told = None;
        while let Err(error) = goal::retire(&node, &scope, &owner).await {
            if told.as_ref() != Some(&error.message) {
                node.log.line(format_args!(
                    "ending the sentinel of {}'s goal; will retry: {}",
                    owner.resident, error.message
                ));
                told = Some(error.message.clone());
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

mod words {
    use fastctx_types::MachineName;

    pub(super) const NOT_SET_UP: &str = "Sentinels are not set up for this account.";
    pub(super) const NO_KEY: &str = "The sentinel's model key is not set.";

    pub(super) fn home_offline(home: &MachineName) -> String {
        format!("The sentinel's home {home} is offline.")
    }

    pub(super) fn home_unusable(home: &MachineName) -> String {
        format!("The sentinel's home {home} cannot be used.")
    }
}
