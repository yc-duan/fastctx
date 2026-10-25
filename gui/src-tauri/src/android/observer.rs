//! Executes the page's explicit notification policy without a WebView,
//! following the World as the hubs push what is afoot on each machine, so a
//! question asked, what an agent told the user, or a turn the user's words
//! began that ended without telling them anything is told the moment it
//! reaches this phone (`world-gui.md`, 对话; `world-voice.md`). State
//! survives process death; unavailable machines retain their last facts.
//! The page gives the titles, in its words; the body is the agent's own:
//! what it asks, what it told, or the words that ended its turn. A question
//! asked without waiting with a few answers carries them as buttons, each
//! with the words that answer it, sent from the notification without
//! opening the app.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_client::World;
use fastctx_platform::{Permissions, publish};
use fastctx_types::residents::{Activity, Asked, AskedFor, Now, Origin};
use fastctx_types::town::{MachineWho, ResidentNow, WorldWho};
use serde::{Deserialize, Serialize};

use super::bridge::Bridge;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Policy {
    enabled: bool,
    rules: Vec<Rule>,
    #[serde(default)]
    agent_names: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Rule {
    when: Transition,
    user_started_only: bool,
    title: Title,
    /// The body of a turn that compacted the conversation and said nothing
    /// else, in the page's words.
    #[serde(default)]
    compacted: Option<String>,
}

/// A title, or one for each kind: each thing an agent may ask (`AskedFor`,
/// and `waiting` for a host that only says it waits), or each way it tells
/// the user something worth a call. A kind with no title is not called about.
#[derive(Deserialize)]
#[serde(untagged)]
enum Title {
    Text(String),
    Kinds(BTreeMap<String, String>),
}

impl Title {
    fn template(&self, kind: Option<&str>) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Kinds(kinds) => kinds.get(kind?).map(String::as_str),
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Transition {
    Asked,
    Told,
    Finished,
}

/// A kind as a title is keyed by: what is serialized as a plain string.
fn kind_of(value: impl Serialize) -> Option<String> {
    serde_json::to_value(value).ok()?.as_str().map(String::from)
}

/// Whether the turn that just ended stands for an answer: one the user's
/// words began that told them nothing through `message_user`. A node that
/// does not say whose words began it is taken as before
/// (`gui/src/app/presence.ts`, `endedUntold`).
fn ended_untold(now: &Now) -> bool {
    now.last_heard_user != Some(false)
        && now.told.as_ref().and_then(|told| told.turn.as_ref()).is_none_or(|turn| now.last_turn.as_ref() != Some(turn))
}

/// The most answers a notification offers as buttons: what Android shows.
const ANSWER_BUTTONS: usize = 3;

/// The answers a question offers as buttons, with the words that give each.
fn answers(asked: &Asked) -> Vec<(String, String)> {
    if asked.waits || asked.what != AskedFor::Answer || asked.options.len() > ANSWER_BUTTONS {
        return vec![];
    }
    (0..asked.options.len()).filter_map(|choice| Some((asked.options[choice].clone(), asked.reply(choice)?))).collect()
}

/// What a notification about a resident's question is keyed by: a new
/// question is a new call (`gui/src/app/presence.ts`, `askedKey`).
fn asked_key(resident: &ResidentNow) -> Option<String> {
    let now = resident.now.as_ref()?;
    now.asked.as_ref().map(|asked| asked.id.clone()).or_else(|| now.waiting_for_user.then(|| "waiting".into()))
}

pub(super) fn validate(json: &str) -> Result<(), String> {
    let policy: Policy = serde_json::from_str(json).map_err(|error| error.to_string())?;
    for (index, rule) in policy.rules.iter().enumerate() {
        let untitled = match &rule.title {
            Title::Text(text) => text.is_empty(),
            Title::Kinds(kinds) => kinds.is_empty() || kinds.values().any(String::is_empty),
        };
        if untitled {
            return Err("a notification rule needs a title".into());
        }
        if policy.rules[..index].iter().any(|other| other.when == rule.when) {
            return Err("only one rule for each transition may be installed".into());
        }
    }
    Ok(())
}

#[derive(Default, Deserialize, Serialize)]
struct Observed {
    world: Option<String>,
    learned: bool,
    residents: BTreeMap<String, Seen>,
}

#[derive(Deserialize, Serialize)]
struct Seen {
    machine: String,
    #[serde(default)]
    asked: Option<String>,
    busy: bool,
    #[serde(default)]
    told: Option<String>,
}

/// A resident as the observations know it: the same for every spelling of
/// its machine.
fn key_of(about: &fastctx_types::residents::About) -> String {
    fastctx_types::Address { machine: about.machine.clone(), resident: about.id.clone() }.key()
}

/// A title from the page's words, `{who}` standing for who it is about.
fn format(template: &str, resident: &ResidentNow, policy: &Policy) -> String {
    let agent = resident.about.host.agent.to_string();
    let agent = policy.agent_names.get(&agent).map(String::as_str).unwrap_or(&agent);
    template.replacen("{who}", &format!("{agent} · {}", resident.about.machine), 1)
}

fn observe(who: WorldWho, bridge: &Bridge, observed: &mut Observed) -> Result<(), String> {
    let policy: Policy = serde_json::from_str(&bridge.string("configuration")?).map_err(|error| error.to_string())?;
    if policy.rules.is_empty() {
        return Ok(());
    }
    for machine in who.machines {
        let MachineWho::Answered { reply, .. } = machine else { continue };
        let residents: Vec<_> = reply.residents.into_iter().filter(|resident| resident.removed.is_none()).collect();
        let names: Vec<_> = residents.iter().map(|r| key_of(&r.about)).collect();
        let removed: Vec<_> = observed
            .residents
            .iter()
            .filter(|(key, seen)| {
                fastctx_types::MachineName::try_from(seen.machine.as_str()).is_ok_and(|seen| seen == reply.machine)
                    && !names.contains(key)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in removed {
            bridge.action("dismissNotice", &format!("{key}:asked"))?;
            observed.residents.remove(&key);
        }
        for resident in residents {
            let key = key_of(&resident.about);
            let now = Seen {
                machine: resident.about.machine.to_string(),
                asked: asked_key(&resident),
                busy: resident.now.as_ref().is_some_and(|now| now.activity == Activity::Busy),
                told: resident.now.as_ref().and_then(|now| now.told.as_ref()).map(|told| told.id.clone()),
            };
            let before = observed.residents.get(&key);
            // What a notification called for no longer holds: it is taken back.
            if before.is_some_and(|was| was.asked.is_some() && was.asked != now.asked) {
                bridge.action("dismissNotice", &format!("{key}:asked"))?;
            }
            let transition = if now.asked.is_some() && before.is_none_or(|was| was.asked != now.asked) {
                Some(Transition::Asked)
            } else if now.told.is_some() && before.is_some_and(|was| was.told != now.told) {
                Some(Transition::Told)
            } else if before.is_some_and(|was| was.busy)
                && !now.busy
                && now.asked.is_none()
                && resident.now.as_ref().is_some_and(ended_untold)
            {
                Some(Transition::Finished)
            } else {
                None
            };
            if observed.learned
                && policy.enabled
                && !bridge.focused()?
                && let Some(rule) = policy.rules.iter().find(|rule| {
                    Some(rule.when) == transition
                        && (!rule.user_started_only
                            || (resident.about.world_started && matches!(resident.about.origin, Origin::User)))
                })
            {
                let asked = resident.now.as_ref().and_then(|now| now.asked.as_ref());
                let told = resident.now.as_ref().and_then(|now| now.told.as_ref());
                let (id, body, version, answers) = match rule.when {
                    Transition::Asked => (
                        format!("{key}:asked"),
                        asked.map(|asked| asked.text.clone()).unwrap_or_default(),
                        now.asked.clone(),
                        asked.map(answers).unwrap_or_default(),
                    ),
                    Transition::Told => (
                        format!("{key}:told"),
                        told.map(|told| told.text.clone()).unwrap_or_default(),
                        now.told.clone(),
                        vec![],
                    ),
                    Transition::Finished => (
                        format!("{key}:finished"),
                        resident
                            .now
                            .as_ref()
                            .and_then(|now| match (&rule.compacted, now.last_compacted) {
                                (Some(compacted), true) => Some(compacted.clone()),
                                _ => now.last_said.clone(),
                            })
                            .unwrap_or_default(),
                        resident.now.as_ref().map(|now| now.since.to_string()),
                        vec![],
                    ),
                };
                let route = format!("#/talk/{}%40{}", resident.about.id, resident.about.machine);
                let answers: Vec<_> = answers
                    .into_iter()
                    .map(|(label, words)| serde_json::json!({"label": label, "words": words}))
                    .collect();
                let kind = match rule.when {
                    Transition::Asked => asked.map_or(Some("waiting".into()), |asked| kind_of(asked.what)),
                    Transition::Told => told.and_then(|told| kind_of(told.status)),
                    Transition::Finished => None,
                };
                if let Some(title) = rule.title.template(kind.as_deref()) {
                    bridge.string_arg(
                        "showNotice",
                        &serde_json::json!({
                            "id": id, "title": format(title, &resident, &policy), "body": body, "route": route,
                            "version": version, "answers": answers,
                            "to": {"resident": resident.about.id, "machine": resident.about.machine},
                        })
                        .to_string(),
                    )?;
                }
            }
            observed.residents.insert(key, now);
        }
    }
    observed.learned = true;
    Ok(())
}

pub(super) async fn run(world: World, directory: PathBuf, bridge: Arc<Bridge>) {
    let path = directory.join("notification-observations.json");
    let loaded = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Observed::default()),
        Err(error) => Err(error.to_string()),
    };
    let mut observed = match loaded {
        Ok(observed) => observed,
        Err(error) => {
            super::bridge::report(format!("reading notification observations: {error}"));
            return;
        }
    };
    let mut saved = serde_json::to_vec(&observed).expect("notification observations");
    let mut last_error = None;
    let mut again = fastctx_client::Again::default();
    loop {
        let mut watch = match world.watch_world().await {
            Ok(watch) => Some(watch),
            Err(error) => {
                report(&bridge, &mut last_error, Some(again.broke(error.to_string()).why));
                None
            }
        };
        while let Some(watching) = &mut watch {
            let now = match watching.next().await {
                Ok(now) => now,
                Err(error) => {
                    report(&bridge, &mut last_error, Some(again.broke(error.to_string()).why));
                    break;
                }
            };
            again.came();
            let state = world.device().expect("Android device").state();
            // The World this phone sees is the hubs it is logged in to.
            let hubs = (!state.hubs.is_empty())
                .then(|| state.hubs.iter().map(|hub| hub.hub.to_string()).collect::<Vec<_>>().join(","));
            let result = (|| {
                if observed.world != hubs {
                    for key in observed.residents.keys() {
                        bridge.action("dismissNotice", &format!("{key}:asked"))?;
                        bridge.action("dismissNotice", &format!("{key}:told"))?;
                        bridge.action("dismissNotice", &format!("{key}:finished"))?;
                    }
                    observed = Observed { world: hubs.clone(), ..Observed::default() };
                }
                observe(now.who, &bridge, &mut observed)?;
                let bytes = serde_json::to_vec(&observed).map_err(|error| error.to_string())?;
                if saved != bytes {
                    publish(&path, &bytes, Permissions::Private).map_err(|error| error.to_string())?;
                    saved = bytes;
                }
                Ok::<_, String>(())
            })();
            report(&bridge, &mut last_error, result.err());
        }
        again.pause(&world, None).await;
    }
}

/// Tells the page what went wrong last, or that nothing is wrong now, once
/// each time that changes.
fn report(bridge: &Bridge, last_error: &mut Option<String>, error: Option<String>) {
    if error == *last_error {
        return;
    }
    if let Some(error) = &error {
        super::bridge::report(error.clone());
    } else if let Err(error) = bridge.action("reportError", "") {
        eprintln!("{error}");
    }
    *last_error = error;
}
