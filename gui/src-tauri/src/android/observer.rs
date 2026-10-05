//! Executes the page's explicit notification policy without a WebView.
//! State survives process death; unavailable machines retain their last facts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fastctx_client::World;
use fastctx_platform::{Permissions, publish};
use fastctx_types::residents::{Activity, Origin};
use fastctx_types::town::{MachineWho, ResidentNow};
use fastctx_types::world::LinkState;
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
    title: String,
    body: Body,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Body {
    Text(String),
    Cases(BodyCases),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyCases {
    neither: String,
    town: String,
    last: String,
    both: String,
}

impl Body {
    fn template(&self, resident: &ResidentNow) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Cases(cases) => match (
                resident.town.is_some(),
                resident.last_action.as_ref().is_some_and(|action| !action.what.is_empty()),
            ) {
                (false, false) => &cases.neither,
                (true, false) => &cases.town,
                (false, true) => &cases.last,
                (true, true) => &cases.both,
            },
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Transition {
    Waiting,
    Finished,
}

pub(super) fn validate(json: &str) -> Result<(), String> {
    let policy: Policy = serde_json::from_str(json).map_err(|error| error.to_string())?;
    for (index, rule) in policy.rules.iter().enumerate() {
        if rule.title.is_empty() {
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
    waiting: bool,
    busy: bool,
}

fn format(template: &str, resident: &ResidentNow, policy: &Policy) -> String {
    let agent = resident.about.host.agent.to_string();
    let agent = policy.agent_names.get(&agent).map(String::as_str).unwrap_or(&agent);
    let who = format!("{agent} · {}", resident.about.machine);
    // Substitutions are made once, so user content containing a placeholder
    // cannot turn into a second template expression.
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else { break };
        let replacement = match &rest[..=end] {
            "{who}" => Some(who.as_str()),
            "{town}" => Some(resident.town.as_ref().map(|town| town.as_str()).unwrap_or("")),
            "{last}" => Some(resident.last_action.as_ref().map(|action| action.what.as_str()).unwrap_or("")),
            _ => None,
        };
        result.push_str(replacement.unwrap_or(&rest[..=end]));
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

async fn observe(world: &World, bridge: &Bridge, observed: &mut Observed) -> Result<(), String> {
    let policy: Policy = serde_json::from_str(&bridge.string("configuration")?).map_err(|error| error.to_string())?;
    if policy.rules.is_empty() {
        return Ok(());
    }
    let who = world.who(None, true).await.map_err(|error| error.to_string())?;
    for machine in who.machines {
        let MachineWho::Answered { reply } = machine else { continue };
        let names: Vec<_> = reply.residents.iter().map(|r| format!("{}@{}", r.about.id, r.about.machine)).collect();
        let removed: Vec<_> = observed
            .residents
            .iter()
            .filter(|(key, seen)| seen.machine == reply.machine.as_str() && !names.contains(key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in removed {
            bridge.action("dismissNotice", &format!("{key}:waiting"))?;
            observed.residents.remove(&key);
        }
        for resident in reply.residents {
            let key = format!("{}@{}", resident.about.id, resident.about.machine);
            let now = Seen {
                machine: resident.about.machine.to_string(),
                waiting: resident.now.as_ref().is_some_and(|now| now.waiting_for_user),
                busy: resident.now.as_ref().is_some_and(|now| now.activity == Activity::Busy),
            };
            if !now.waiting {
                bridge.action("dismissNotice", &format!("{key}:waiting"))?;
            }
            let before = observed.residents.get(&key);
            let transition = if now.waiting && !before.is_some_and(|was| was.waiting) {
                Some(Transition::Waiting)
            } else if before.is_some_and(|was| was.busy) && !now.busy && !now.waiting {
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
                let id = match rule.when {
                    Transition::Waiting => format!("{key}:waiting"),
                    Transition::Finished => format!("{key}:finished"),
                };
                let route = format!("#/talk/{}%40{}", resident.about.id, resident.about.machine);
                bridge.string_arg(
                    "showNotice",
                    &serde_json::json!({
                        "id": id, "title": format(&rule.title, &resident, &policy),
                        "body": format(rule.body.template(&resident), &resident, &policy), "route": route,
                        "version": resident.now.as_ref().map(|now| now.since.to_string()),
                    })
                    .to_string(),
                )?;
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
    loop {
        let state = world.device().expect("Android device").state();
        let result = async {
            if observed.world != state.world {
                for key in observed.residents.keys() {
                    bridge.action("dismissNotice", &format!("{key}:waiting"))?;
                    bridge.action("dismissNotice", &format!("{key}:finished"))?;
                }
                observed = Observed { world: state.world.clone(), ..Observed::default() };
            }
            if matches!(state.link, LinkState::Online { .. }) {
                observe(&world, &bridge, &mut observed).await?;
            }
            let bytes = serde_json::to_vec(&observed).map_err(|error| error.to_string())?;
            if saved != bytes {
                publish(&path, &bytes, Permissions::Private).map_err(|error| error.to_string())?;
                saved = bytes;
            }
            Ok::<_, String>(())
        }
        .await;
        let error = result.err();
        if error != last_error {
            if let Some(error) = &error {
                super::bridge::report(error.clone());
            } else if let Err(error) = bridge.action("reportError", "") {
                eprintln!("{error}");
            }
            last_error = error;
        }
        // The same sampling interval as the page's central World store.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
