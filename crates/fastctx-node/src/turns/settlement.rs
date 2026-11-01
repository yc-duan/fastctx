//! Recoverable terminal effects are separate from publication and late reader
//! additions. Answer debts precede clearing the resident's running obligation.
use crate::{
    Node,
    delivery::{self, terminals},
    records,
};
use fastctx_storage::Priority;
use fastctx_types::{
    Address, ResidentId,
    residents::{MessageKind, MessageLine, Notice, Origin, Trouble},
    session::{Happened, SessionEvent, Speaker, TurnHow},
};
use std::{io, sync::Arc};

pub(crate) async fn settle_terminal_locked(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: &str,
    first: bool,
) -> io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let directory = node.world.conversation(id);
    let Some(header) = delivery::terminal_header(&directory, turn).await? else { return Ok(()) };
    let (writing, key, colleague, label) = (
        directory.clone(),
        turn.to_string(),
        Address { machine: node.machine(), resident: id.clone() },
        match &about.origin {
            Origin::Colleague { label, .. } => label.clone(),
            Origin::User => String::new(),
        },
    );
    fastctx_platform::task::spawn_blocking(move || super::answers::create(&writing, &key, &colleague, &label))
        .await
        .map_err(io::Error::other)??;
    let (pay_node, pay_id, pay_directory) = (node.clone(), id.clone(), directory.clone());
    fastctx_platform::task::spawn(async move { crate::outbox::pay(&pay_node, &pay_id, &pay_directory).await });
    if header.effects_complete {
        return complete(directory, turn.to_string()).await;
    }
    let (working, resident, key, identity) = (node.clone(), id.clone(), turn.to_string(), about.clone());
    let held = node.residents.keep_speaking(id)?;
    let (header, end, recorded, current, inputs) = fastctx_platform::task::spawn_blocking(move || {
        let _held = held;
        let directory = working.world.conversation(&resident);
        let (header, end) =
            delivery::outcome(&directory, &key)?.ok_or_else(|| io::Error::other("terminal disappeared"))?;
        let usage = records::read(&directory, |rows| terminals::content(rows, &header).map(|content| content.usage))?;
        if let Some(usage) = usage {
            crate::usage::record_terminal(&working, &directory, &key, usage)?;
        }
        let accepted = delivery::handovers::for_turn(&directory, &key)?;
        let current = working.residents.now(&resident).is_some_and(|now| {
            now.turn.as_deref() == Some(&key)
                || (key.is_empty() && now.turn.is_none() && now.activity == fastctx_types::residents::Activity::Busy)
        });
        let trouble = match (&end.how, &end.reason) {
            (TurnHow::Completed, _) => None,
            (TurnHow::Interrupted, _) if crate::stops::requested_turn(&working, &resident, &key)? => None,
            (how, Some(reason)) => Some(Trouble { at: header.at, text: format!("turn {how}: {reason}") }),
            (how, None) => Some(Trouble { at: header.at, text: format!("turn {how}") }),
        };
        let heard_user = accepted.iter().flat_map(|handover| &handover.inputs).any(|input| {
            matches!(input.delivery.notice, Notice::Words { from: Speaker::User { .. }, .. } | Notice::Attached { .. })
        });
        if current && matches!(end.how, TurnHow::Interrupted) {
            working.interrupts.raise(&Address { machine: working.machine(), resident: resident.clone() });
        }
        working.residents.take_owed(&working.world, &resident, &key, &working.machine(), trouble, heard_user)?;
        if current {
            crate::turn_slots::ended(&working, &resident, &identity, &end);
        }
        let mut inputs = Vec::new();
        for handover in accepted {
            for input in handover.inputs {
                crate::speech::heard(&working, &resident, &input.delivery, &key)?;
                inputs.push(input.delivery.id);
            }
        }
        let event = SessionEvent {
            at: header.ended_at.unwrap_or(header.at),
            happened: Happened::TurnEnded { turn: key.clone(), end: end.clone() },
        };
        let recorded = delivery::event(&directory, &format!("ended:{key}"), &event)?;
        if first {
            crate::codex_title::maybe(&working, &resident, &identity);
        }
        Ok::<_, io::Error>((header, end, recorded, current, inputs))
    })
    .await
    .map_err(io::Error::other)??;
    if about.host.agent.as_str() == "claude" {
        crate::claude::turn_facts::settled(&directory, turn).await?;
    }
    crate::turn_evidence::ended(node, id, turn);
    node.idle_probes.touched(id);
    node.watchers.changed.notify_one();
    if recorded {
        if current || !turn.is_empty() {
            crate::desk::turn_ended(node, id, turn);
        }
        node.watchers.publish(
            id,
            &SessionEvent {
                at: header.ended_at.unwrap_or(header.at),
                happened: Happened::TurnEnded { turn: turn.to_string(), end: end.clone() },
            },
        );
    }
    let settled = crate::composer::turn_ended(node, id, turn, &end.how, header.settles_queue, header.queue_order).await;
    crate::control_queue::schedule(node, id);
    settled.map_err(io::Error::other)?;
    let (settling, working) = (directory.clone(), node.clone());
    let held = node.residents.keep_speaking(id)?;
    fastctx_platform::task::spawn_blocking(move || {
        let _held = held;
        for input in inputs {
            crate::inbox::settle(&working, &settling, &input)?;
        }
        Ok::<(), io::Error>(())
    })
    .await
    .map_err(io::Error::other)??;
    if matches!(about.origin, Origin::Colleague { .. }) {
        let (writing, key) = (directory.clone(), turn.to_string());
        fastctx_platform::task::spawn_blocking(move || {
            let (_, _, readers) =
                terminals::read(&writing, &key)?.ok_or_else(|| io::Error::other("terminal disappeared"))?;
            crate::residents::append_message(
                &writing,
                &MessageLine { at: header.at, what: MessageKind::Ended { turn: key, end, owed: readers } },
            )
        })
        .await
        .map_err(io::Error::other)??;
    }
    complete(directory, turn.to_string()).await
}
async fn complete(directory: fastctx_types::layout::ConversationDir, turn: String) -> io::Result<()> {
    fastctx_platform::task::spawn_blocking(move || {
        records::write_at(&directory, Priority::Normal, 8192, move |rows| {
            terminals::effects_complete(rows, &turn)?;
            delivery::handovers::complete_turn(rows, &turn)?;
            terminals::settle_work(rows, &turn).map(|_| ())
        })
    })
    .await
    .map_err(io::Error::other)?
}
