//! Nonblocking questions retain their cards and route to the goal's actual owner.

use crate::Node;
use fastctx_types::goals::Owner;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing};
use fastctx_types::session::Item;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::io;
use std::sync::Arc;

/// A durable question identity prevents replay from reviving an answered card.
/// Claude's synthetic cards also need this record because its transcript has no such message.
pub(crate) fn key(turn: &str, item: &Item) -> Option<String> {
    crate::asked::of_item(item, Timestamp::UNIX_EPOCH).map(|_| format!("async-question:{turn}:{}", item.id))
}

/// Returns true when a requesting conversation owns this question instead of the user.
/// Publication precedes delivery; the existing outbox recovers across restart and disconnection.
pub(crate) fn route(node: &Arc<Node>, id: &ResidentId, item: &Item, at: Timestamp) -> io::Result<bool> {
    let owner = node
        .goals
        .views
        .lock()
        .expect("goal views")
        .get(id)
        .and_then(|view| view.goal.as_ref())
        .filter(|goal| goal.retired.is_none())
        .map(|goal| (goal.owner.clone(), goal.account.clone()));
    let destination = match owner {
        Some((Owner::Conversation { address }, account)) => Some((address, Some(account))),
        Some((Owner::User, _)) => None,
        None => node.residents.about(id).and_then(|(about, _)| match about.origin {
            Origin::Colleague { asked_by, .. } => Some((asked_by, None)),
            Origin::User => None,
        }),
    };
    let Some((requester, account)) = destination else { return Ok(false) };
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let delivery = crate::speech::fixed_id("goal-question", &[&colleague.key(), &item.id, &requester.key()]);
    let text = super::words::question(&colleague, item);
    let questions = item
        .detail
        .as_ref()
        .and_then(|detail| detail.pointer("/fastctx_goal_question/questions").or_else(|| detail.get("questions")))
        .cloned();
    let owing = Owing {
        to: requester,
        delivery: Delivery {
            id: delivery.clone(),
            at,
            notice: Notice::Words {
                from: fastctx_types::session::Speaker::Resident { address: colleague.clone() },
                text,
                goal_report: None,
                goal_question: Some(Box::new(fastctx_types::goals::QuestionDelivery {
                    colleague,
                    id: delivery.to_string(),
                    account,
                    questions,
                })),
            },
        },
    };
    let directory = node.world.conversation(id);
    crate::inbox::owe(&directory, &owing)?;
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move { crate::turns::pay(&node, &id, &directory).await });
    Ok(true)
}
