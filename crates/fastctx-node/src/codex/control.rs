//! The user's controls of a World-held Codex resident. A fork has a new
//! identity and no inherited obligation to the original resident's asker.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, Activity, Now, Origin};
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

fn thread(node: &Node, resident: &ResidentId) -> Result<String, Failure> {
    node.residents
        .about(resident)
        .and_then(|(about, _)| about.session)
        .ok_or_else(|| Failure::new(format!("{resident} has no Codex thread")))
}

pub(super) fn reported(node: &Node, resident: &ResidentId, thread: &str, event: &SessionEvent) -> std::io::Result<()> {
    if let Happened::Changed { change: SessionChange::Settings { settings }, .. } = &event.happened
        && let Some(model) = settings.pointer("/threadSettings/model").and_then(serde_json::Value::as_str)
    {
        node.residents.set_session(&node.world, resident, thread, Some(model.into()), None)?;
    }
    Ok(())
}

pub async fn offers(node: &Arc<Node>, resident: &ResidentId) -> Result<Offers, Failure> {
    node.codex.offers(&thread(node, resident)?).await.map_err(Failure::new)
}

pub async fn control(node: &Arc<Node>, resident: &ResidentId, mut control: Control) -> Result<Controlled, Failure> {
    let thread = thread(node, resident)?;
    if let Control::Answer { request, answer } = control {
        node.codex.answer(&thread, &request, &answer).map_err(Failure::new)?;
        return done("Answer sent. Codex uses the first response it receives; another client may have answered first.");
    }
    if let Control::Command { name, args, path: None } = &control {
        control = match name.as_str() {
            "compact" => {
                no_args(args)?;
                Control::Compact
            }
            "model" => {
                let words: Vec<_> = args.split_whitespace().collect();
                match words.as_slice() {
                    [] => {
                        return done(
                            &serde_json::to_string(&offers(node, resident).await?)
                                .map_err(|error| Failure::new(error.to_string()))?,
                        );
                    }
                    [model] => Control::Model { model: Some((*model).into()), effort: None },
                    [model, effort] => Control::Model { model: Some((*model).into()), effort: Some((*effort).into()) },
                    _ => return Err(Failure::new("/model takes <model> [effort]")),
                }
            }
            "rename" => Control::Rename { name: args.clone() },
            "fork" => {
                no_args(args)?;
                Control::Fork { through: None }
            }
            "status" | "usage" | "skills" => {
                no_args(args)?;
                let offers = offers(node, resident).await?;
                let value = match name.as_str() {
                    "usage" => serde_json::to_value(offers.usage),
                    "skills" => serde_json::to_value(
                        offers.commands.into_iter().filter(|command| command.path.is_some()).collect::<Vec<_>>(),
                    ),
                    _ => serde_json::to_value(offers),
                }
                .map_err(|error| Failure::new(error.to_string()))?;
                return done(&value.to_string());
            }
            "mcp" => {
                if !matches!(args.trim(), "" | "verbose") {
                    return Err(Failure::new("/mcp takes only the optional argument verbose"));
                }
                return done(&node.codex.mcp(&thread).await.map_err(Failure::new)?.to_string());
            }
            _ => control,
        };
    }
    let speaking = node.residents.speaking(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let speaking = speaking.lock().await;
    let _starting =
        node.handoff.start_turn().ok_or_else(|| Failure::new(crate::handoff::Handoff::waiting_why(node)))?;
    match control {
        Control::Compact => {
            node.codex.compact(&thread).await.map_err(Failure::new)?;
            done("Compaction started; its completion appears in the conversation.")
        }
        Control::Model { model, effort } => {
            node.codex.select_model(&thread, model, effort).await.map_err(Failure::new)?;
            done("Selection saved for the next turn started through the World. The running turn keeps its settings.")
        }
        Control::Rename { name } => {
            node.codex.rename(&thread, &name).await.map_err(Failure::new)?;
            Ok(Controlled::Done { said: None })
        }
        Control::Fork { through } => fork(node, resident, &thread, through).await,
        Control::Rewind { from } => {
            node.codex.rewind(&thread, &from).await.map_err(Failure::new)?;
            drop(speaking);
            node.codex.flush_events(&thread).await.map_err(Failure::new)?;
            done("Conversation rewound. Files changed by discarded turns have not been restored.")
        }
        Control::Command { name, args, path: None } if name == "review" => {
            let answer = node.codex.review(&thread, &args).await.map_err(Failure::new)?;
            done(&format!(
                "Review started using Codex's current review/default settings. Pending model choices apply to the next ordinary turn. {answer}"
            ))
        }
        Control::Command { name, args, path } => {
            let sent = node.codex.skill(&thread, &name, path.as_deref(), &args).await.map_err(Failure::new)?;
            done(&format!("Skill input {} turn {}", if sent.joined { "joined" } else { "started" }, sent.turn))
        }
        Control::Answer { .. } => unreachable!("answers do not acquire the speech lock"),
    }
}

async fn fork(
    node: &Arc<Node>,
    parent: &ResidentId,
    thread: &str,
    through: Option<String>,
) -> Result<Controlled, Failure> {
    let (source, town) = node.residents.about(parent).ok_or_else(|| Failure::new("unknown source resident"))?;
    let id = node.residents.new_id(&source.host.agent)?;
    let (spec, heard) =
        crate::colleagues::thread_spec(node, &id, &Origin::User, town.as_ref(), source.works_on.as_ref()).await?;
    let (fork, model) = node.codex.fork(thread, through.as_deref(), spec).await.map_err(Failure::new)?;
    let at = Timestamp::now();
    let about = About {
        id: id.clone(),
        host: source.host,
        machine: node.machine(),
        born_at: at,
        origin: Origin::User,
        session: Some(fork.clone()),
        world_started: true,
        model,
        acting: source.acting,
        works_on: source.works_on,
    };
    crate::colleagues::setting::inherit(node, parent, &id)?;
    let now = Now {
        activity: Activity::Idle,
        since: at,
        turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
    };
    node.residents.create(&node.world, about, town, now).map_err(|error| {
        Failure::new(format!("Codex created thread {fork}, but recording its resident {id} failed: {error}"))
    })?;
    node.residents.set_heard(&id, heard);
    let event = SessionEvent {
        at,
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Forked {
                source: Address { machine: node.machine(), resident: parent.clone() },
                thread: thread.into(),
                through,
            },
        },
    };
    crate::turns::apply(node, &id, event).await.map_err(|error| Failure::new(error.to_string()))?;
    crate::codex::claim(node, &fork, &id).await;
    Ok(Controlled::Forked { resident: Address { machine: node.machine(), resident: id } })
}

fn no_args(args: &str) -> Result<(), Failure> {
    if args.trim().is_empty() { Ok(()) } else { Err(Failure::new("this command takes no arguments")) }
}

fn done(said: &str) -> Result<Controlled, Failure> {
    Ok(Controlled::Done { said: Some(said.into()) })
}
