//! Read delivery custody in its forward order, so a handover cannot disappear between reads.

use crate::Node;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::turn_evidence::ConversationDebt;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::io;
use std::sync::Arc;

fn deliveries(node: &Node, resident: &ResidentId, requester: &Address) -> io::Result<ConversationDebt> {
    let directory = node.world.conversation(resident);
    let names = node.names();
    let matching = |input: fastctx_types::residents::Pending| {
        input
            .readers
            .iter()
            .any(|reader| super::identity::same(&names, reader, requester))
            .then(|| (input.delivery.id.to_string(), input.delivery.at))
    };
    let (mut pending, mut uncertain) = (Vec::new(), Vec::new());
    crate::inbox::pages(&directory, |page| {
        pending.extend(page.into_iter().filter_map(matching));
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    crate::inbox::uncertain_pages(&directory, |page| {
        uncertain.extend(page.into_iter().filter_map(matching));
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    let now = node.residents.now(resident).ok_or_else(|| io::Error::other("conversation state is unavailable"))?;
    let running = now
        .owed
        .iter()
        .filter(|(_, readers)| readers.iter().any(|reader| super::identity::same(&names, reader, requester)))
        .map(|(turn, _)| (turn.clone(), now.since))
        .collect();
    let mut answers = Vec::new();
    crate::inbox::each_owing(&directory, |owing| {
        if super::identity::same(&names, &owing.to, requester)
            && owing.delivery.notice.along().is_none()
            && !matches!(owing.delivery.notice, fastctx_types::residents::Notice::Words { goal_report: Some(_), .. })
        {
            answers.push((owing.delivery.id.to_string(), owing.delivery.at));
        }
    })?;
    Ok(ConversationDebt { observed_at: Timestamp::now(), goal: None, pending, running, answers, uncertain })
}

pub(crate) async fn here(
    node: &Arc<Node>,
    acting: &Acting,
    resident: &ResidentId,
    requester: &Address,
) -> Result<ConversationDebt, Failure> {
    let goal = crate::goals::waiting::debt(node, acting, resident, requester).await?;
    let mut debt = deliveries(node, resident, requester).map_err(|error| Failure::new(error.to_string()))?;
    debt.goal = goal.map(Box::new);
    Ok(debt)
}

pub(crate) async fn read(
    node: &Arc<Node>,
    acting: &Acting,
    colleague: &Address,
    requester: &Address,
) -> Result<ConversationDebt, Failure> {
    if node.is_me(&colleague.machine) {
        return here(node, acting, &colleague.resident, requester).await;
    }
    let machine = (&colleague.machine).into();
    let facts = node
        .call(acting, &machine, Purpose::Look, RemoteCall::Facts)
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    if !matches!(facts, RemoteReply::Facts(facts) if facts.goal_waits) {
        return Err(Failure::new(crate::goals::older_goal_waits(&colleague.machine)));
    }
    match node
        .call(
            acting,
            &machine,
            Purpose::Look,
            RemoteCall::ConversationDebt { resident: colleague.resident.clone(), requester: requester.clone() },
        )
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::ConversationDebt { debt } => Ok(debt),
        _ => Err(Failure::new("expected outstanding conversation deliveries")),
    }
}
