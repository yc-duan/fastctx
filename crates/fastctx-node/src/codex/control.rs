//! The user's controls of a World-held Codex resident. A fork has a new
//! identity and no inherited obligation to the original resident's asker.

use std::sync::Arc;

use fastctx_types::agents::{AgentLimits, Fact};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, Activity, Now, Origin};
use fastctx_types::session::control::{Control, Controlled, Limit, Offers};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use fastctx_hosts::codex::CodexHost;

use crate::Node;

/// The resident's thread, and the app-server of its home that holds it.
fn thread(node: &Node, resident: &ResidentId) -> Result<(Arc<CodexHost>, String), Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new(format!("{resident} is not here")))?;
    crate::homes::available(node, &about)?;
    let host = node.codex.of(node, &about)?;
    about.session.map(|thread| (host, thread)).ok_or_else(|| Failure::new(format!("{resident} has no Codex thread")))
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
    let (codex, thread) = thread(node, resident)?;
    let mut offers = codex.offers(&thread).await.map_err(Failure::new)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    if crate::assists::assisting(node, &about.acting) {
        if crate::assists::host::kept(node, resident)? != Some(true) {
            offers.controls.clear();
            offers.commands.clear();
            return Ok(offers);
        }
        offers
            .commands
            .retain(|command| command.path.is_none() && fastctx_hosts::codex::assisted::command_allowed(&command.name));
    }
    // A thread Codex has not reported on since it restarted has the model
    // it last reported in the resident's record.
    if offers.model.is_none() {
        offers.model = node.residents.about(resident).and_then(|(about, _)| about.model);
    }
    let dir = codex.config_home().to_path_buf();
    let kept = {
        let node = node.clone();
        tokio::task::spawn_blocking(move || crate::logins::limits::kept_of(&node, &dir))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("reading Codex's limits: {error}")))?
    };
    if let (Some(usage), Fact::Known { value }) = (offers.usage.as_mut(), kept) {
        usage.limits = limits(&value);
        usage.limits_at = Some(value.observed_at);
    }
    Ok(offers)
}

/// The login's limits as the conversation shows them, each window named
/// with its length.
fn limits(kept: &AgentLimits) -> Vec<Limit> {
    kept.windows
        .iter()
        .map(|window| Limit {
            name: match window.minutes {
                Some(minutes) => format!("{} {minutes}m", window.name),
                None => window.name.clone(),
            },
            used_percent: serde_json::Number::from_f64(f64::from(window.used_tenths) / 10.0),
            resets_at: window.resets_at,
        })
        .collect()
}

pub async fn control(node: &Arc<Node>, resident: &ResidentId, mut control: Control) -> Result<Controlled, Failure> {
    let (codex, thread) = thread(node, resident)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    if crate::assists::assisting(node, &about.acting) {
        crate::assists::host::require(node, resident)?;
    }
    if crate::assists::assisting(node, &about.acting)
        && let Control::Command { name, path, .. } = &control
        && (path.is_some() || !fastctx_hosts::codex::assisted::command_allowed(name))
    {
        return Err(Failure::not_run(fastctx_hosts::codex::assisted::COMMAND_REFUSED));
    }
    if let Control::Answer { request, answer } = control {
        codex.answer(&thread, &request, &answer).map_err(Failure::new)?;
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
                        let offers = offers(node, resident).await?;
                        return Ok(Controlled::ModelMenu {
                            models: offers.models,
                            model: offers.model,
                            effort: offers.effort,
                            service_tier: offers.service_tier,
                        });
                    }
                    [model] => Control::Model { model: Some((*model).into()), effort: None },
                    [model, effort] => Control::Model { model: Some((*model).into()), effort: Some((*effort).into()) },
                    _ => return Err(Failure::new("/model takes <model> [effort]")),
                }
            }
            "rename" => Control::Rename { name: args.clone() },
            "fork" => {
                no_args(args)?;
                Control::Fork { through: None, side: false }
            }
            "status" | "usage" | "skills" => {
                no_args(args)?;
                let mut offers = offers(node, resident).await?;
                // The user asked: the account's limits and usage are read
                // from OpenAI now.
                if name != "skills" {
                    let mut fresh = codex.read_usage(&thread).await.map_err(Failure::new)?;
                    if let Some(kept) = &offers.usage {
                        fresh.context = kept.context.clone();
                        fresh.cost_usd = kept.cost_usd.clone();
                        if let (Some(fresh), Some(kept)) = (fresh.raw.as_object_mut(), kept.raw.as_object()) {
                            fresh.extend(kept.iter().map(|(key, value)| (key.clone(), value.clone())));
                        }
                    }
                    offers.usage = Some(fresh);
                }
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
                return done(&codex.mcp(&thread).await.map_err(Failure::new)?.to_string());
            }
            _ => control,
        };
    }
    let speaking = node.residents.speaking(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let speaking = speaking.lock().await;
    let _starting =
        node.handoff.start_turn().ok_or_else(|| Failure::new(crate::handoff::Handoff::waiting_why(node)))?;
    // What starts a turn takes a turn of its login, as words do; joining
    // the turn that runs takes none.
    let admit = || {
        if codex.running_turn(&thread).is_some() {
            return Ok(());
        }
        crate::turn_slots::admit(node, resident, &about).map_err(|_| crate::turn_slots::full())
    };
    match control {
        Control::Compact => {
            admit()?;
            codex.compact(&thread).await.map_err(|error| error.failure())?;
            done("Compaction started; its completion appears in the conversation.")
        }
        Control::Model { model, effort } => {
            codex.select_model(&thread, model, effort).await.map_err(Failure::new)?;
            done("Selection saved for the next turn started through the World. The running turn keeps its settings.")
        }
        Control::Rename { name } => {
            if name.trim().is_empty() {
                return Err(Failure::new("a conversation name cannot be empty"));
            }
            codex.rename(&thread, &name).await.map_err(|error| error.failure())?;
            crate::codex::titled(node, resident, name).await;
            Ok(Controlled::Done { said: None })
        }
        Control::Fork { through, side } => {
            drop(speaking);
            fork(node, resident, &codex, &thread, through, side).await
        }
        Control::Rewind { from } => {
            codex.rewind(&thread, &from).await.map_err(|error| error.failure())?;
            drop(speaking);
            codex.flush_events(&thread).await.map_err(Failure::new)?;
            done("Conversation rewound. Files changed by discarded turns have not been restored.")
        }
        Control::Command { name, args, path: None } if name == "review" => {
            admit()?;
            let answer = codex.review(&thread, &args).await.map_err(|error| error.failure())?;
            done(&format!(
                "Review started using Codex's current review/default settings. Pending model choices apply to the next ordinary turn. {answer}"
            ))
        }
        Control::Command { name, args, path: None } if name == "goal" => {
            if !fastctx_platform::preview() {
                return Err(Failure::not_run(fastctx_types::goals::words::UNOFFERED));
            }
            done(&codex.goal(&thread, &args).await.map_err(Failure::new)?.to_string())
        }
        Control::Command { name, args, path: None } if name == "debug-config" => {
            no_args(&args)?;
            done(&codex.read_config(&thread).await.map_err(Failure::new)?.to_string())
        }
        // A speed is a choice kept for the next turn, like a model: nothing
        // is listed for it.
        Control::Command { name, args, path: None } if name == "tier" && !args.trim().is_empty() => {
            let selected = codex.select_tier(&thread, args.trim(), false).await.map_err(Failure::new)?;
            done(&format!("Service tier {selected} saved for the next World turn."))
        }
        Control::Command { name, args, path } => {
            if path.is_none() {
                let catalog = codex.offers(&thread).await.map_err(Failure::new)?;
                if name == "tier" {
                    if args.trim().is_empty() {
                        let model = catalog.models.iter().find(|model| Some(&model.id) == catalog.model.as_ref());
                        return done(
                            &serde_json::json!({"service_tier":catalog.service_tier,
                            "default_service_tier":model.and_then(|model| model.default_service_tier.as_ref()),
                            "service_tiers":model.map(|model| &model.service_tiers)})
                            .to_string(),
                        );
                    }
                    let selected = codex.select_tier(&thread, args.trim(), false).await.map_err(Failure::new)?;
                    return done(&format!("Service tier {selected} saved for the next World turn."));
                }
                if catalog
                    .commands
                    .iter()
                    .any(|command| command.name == name && command.source.as_deref() == Some("codex"))
                    && let Some(tier) = catalog
                        .models
                        .iter()
                        .find(|model| Some(&model.id) == catalog.model.as_ref())
                        .and_then(|model| model.service_tiers.iter().find(|tier| tier.name.to_lowercase() == name))
                {
                    no_args(&args)?;
                    let selected = codex.select_tier(&thread, &tier.id, true).await.map_err(Failure::new)?;
                    return done(&format!("Service tier {selected} saved for the next World turn."));
                }
            }
            admit()?;
            let sent = codex.skill(&thread, &name, path.as_deref(), &args).await.map_err(Failure::new)?;
            done(&format!("Skill input {} turn {}", if sent.joined { "joined" } else { "started" }, sent.turn))
        }
        Control::Answer { .. } => unreachable!("answers do not acquire the speech lock"),
        Control::CancelControl { .. } => unreachable!("queued controls are cancelled by the node"),
        Control::Agents | Control::AgentMessages { .. } => {
            Err(Failure::new("this host does not offer native agent controls"))
        }
    }
}

async fn fork(
    node: &Arc<Node>,
    parent: &ResidentId,
    codex: &Arc<CodexHost>,
    thread: &str,
    through: Option<String>,
    side: bool,
) -> Result<Controlled, Failure> {
    let gate = node.residents.management(parent)?;
    let source_guard = crate::gate_waits::shared(node, parent, &gate).await?;
    node.residents.active(parent)?;
    let (source, town) = node.residents.about(parent).ok_or_else(|| Failure::new("unknown source resident"))?;

    crate::layers::may_start(node, &source.acting)?;
    let id = node.residents.new_id(&source.host.agent)?;
    let _birth = crate::here::born(node, &id, source.acting.clone(), &Origin::User);
    crate::assists::host::inherit(node, codex.assisted_cwd().is_some(), &id)?;
    crate::session_options::keep(node, &id, &crate::session_options::read(node, parent)?)?;
    crate::focus::open(node, &node.world.conversation(&id), &source.acting, &source.focus).await?;
    let (spec, heard) =
        crate::colleagues::thread_spec(node, &id, &Origin::User, town.as_ref(), &source.acting, source.home.as_deref())
            .await?;
    // Until it is claimed below, reports of threads nobody claims are kept.
    let _starting = crate::codex::starting(node);
    let (fork, model) = codex.fork(thread, through.as_deref(), spec).await.map_err(|error| error.failure())?;
    let at = Timestamp::now();
    let about = About {
        forked_from: Some(Address { machine: node.machine(), resident: parent.clone() }),
        side,
        mcp_off: source.mcp_off.clone(),
        id: id.clone(),
        host: source.host,
        machine: node.machine(),
        born_at: at,
        origin: Origin::User,
        session: Some(fork.clone()),
        world_started: true,
        native_resumed: false,
        model,
        acting: source.acting,
        focus: source.focus,
        home: source.home,
        callee: None,
    };
    let now = Now {
        activity: Activity::Idle,
        since: at,
        turn: None,
        last_turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    };
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents.create(&node.world, about, town, now).map_err(|error| {
        Failure::new(format!("Codex created thread {fork}, but recording its resident {id} failed: {error}"))
    })?;
    let inherited =
        codex.history(&fork, &Address { machine: node.machine(), resident: id.clone() }).await.map_err(Failure::new)?;
    crate::attachments::inherit(node, parent, &id, &inherited)?;
    crate::context::input::inherit(&node.world.conversation(parent), &node.world.conversation(&id), &inherited)
        .map_err(|error| Failure::new(error.to_string()))?;
    drop(source_guard);
    crate::context::instructed(node, &id, &heard).map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_instructed(&id, heard);
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
    crate::focus::forked(node, &id).await?;
    Ok(Controlled::Forked { resident: Address { machine: node.machine(), resident: id } })
}

fn no_args(args: &str) -> Result<(), Failure> {
    if args.trim().is_empty() { Ok(()) } else { Err(Failure::new("this command takes no arguments")) }
}

fn done(said: &str) -> Result<Controlled, Failure> {
    Ok(Controlled::Done { said: Some(said.into()) })
}
