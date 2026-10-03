//! What this machine's town asks of other machines' towns (`super::verbs` answers): a
//! resident's hands on another machine, `fastctx w` across the World, colleagues elsewhere, and
//! finding the machine a resident lives on.

use super::verbs::{ColleagueCall, Guest};
use super::{Letter, Peer};
use crate::control::codex_config::TOOL_TIMEOUT_SECONDS;
use crate::model::{ToolContent, ToolResponse};
use crate::town::core::{Asked, Spoken, Town};
use crate::town::project;
use crate::town::w::{Silence, Silent};
use crate::town::who::{MachineAnswer, Query};
use crate::world::client::NodeOutcome;
use crate::world::messages::{CallBudget, CallOrigin};
use serde_json::{Value, json};
use std::time::Duration;

/// How long a town waits for another machine's town: as long as a host waits for the World
/// tool that made the call, the longest any host is told to (`tool_timeout_sec`).
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(TOOL_TIMEOUT_SECONDS as u64);

/// What another machine said, or why it said nothing: `laptop: offline (since 13:40Z)`.
pub(crate) fn status_line(outcome: &NodeOutcome) -> String {
    match &outcome.message {
        Some(message) => format!("{}: {} ({message})", outcome.node, outcome.status),
        None => format!("{}: {}", outcome.node, outcome.status),
    }
}

/// The response a model reads from another machine's hand. The machine's head note already
/// names it; a failure is prefixed with the machine it happened on.
pub(crate) fn response_of(outcome: NodeOutcome) -> ToolResponse {
    match outcome.response {
        Some(response) if !response.is_error => response,
        Some(response) => {
            let text: Vec<&str> = response
                .content
                .iter()
                .filter_map(|block| match block {
                    ToolContent::Text(text) => Some(text.as_str()),
                    ToolContent::Image { .. } => None,
                })
                .collect();
            ToolResponse::error(format!("{}: {}", outcome.node, text.join("\n")))
        }
        None => ToolResponse::error(status_line(&outcome)),
    }
}

/// The error a failed town call reports.
pub(crate) fn outcome_error(outcome: &NodeOutcome) -> String {
    match outcome.response.as_ref() {
        Some(response) => response
            .content
            .iter()
            .find_map(|block| match block {
                ToolContent::Text(text) => Some(format!("{}: {text}", outcome.node)),
                ToolContent::Image { .. } => None,
            })
            .unwrap_or_else(|| status_line(outcome)),
        None => status_line(outcome),
    }
}

impl Town {
    /// Calls another machine's town. A machine that is offline, forbidden, or silent answers
    /// with a status, not an error.
    pub(crate) async fn call_town(&self, node: &str, verb: &str, args: Value) -> NodeOutcome {
        match self.world().call(verb, node, args, CallBudget::default(), CALL_TIMEOUT, CallOrigin::Tool).await {
            Ok(outcome) => outcome,
            Err(error) => NodeOutcome::status(node, "error", error),
        }
    }

    /// One of a resident's hands on another machine: `hand` is the tool, `what` what it acts
    /// on, `request` the tool's own arguments. The resident's action line is written here, with
    /// the machine the hand reached.
    pub(crate) async fn hand_elsewhere(&self, resident: &str, node: &str, hand: &str, what: &str, request: Value) -> ToolResponse {
        let guest = Guest { resident: resident.to_string(), town: self.town_of(resident).flatten() };
        let mut args = request;
        if let (Value::Object(args), Ok(Value::Object(guest))) = (&mut args, serde_json::to_value(&guest)) {
            args.extend(guest);
        }
        let outcome = self.call_town(node, &format!("hand_{hand}"), args).await;
        let result = match outcome.facts.get("result") {
            Some(Value::String(result)) => result.clone(),
            _ => format!("not done: {}", status_line(&outcome)),
        };
        self.record_action(resident, node, hand, what, &result);
        response_of(outcome)
    }

    /// `ask_colleague` for a resident of this machine: a colleague here, or on the machine it
    /// names. The ask is one of its actions, recorded with where the colleague lives.
    pub(crate) async fn ask_for(&self, requester: &str, node: Option<&str>, prompt: &str, label: &str) -> Result<Asked, String> {
        let town = self.requester_town(requester)?;
        let machine = node.unwrap_or(self.machine()).to_string();
        let asked = if machine == self.machine() {
            self.ask_colleague(&Peer::new(self.machine(), requester), town, prompt, label).await
        } else {
            let call = ColleagueCall { requester: requester.to_string(), town, prompt: prompt.to_string(), label: label.to_string() };
            let outcome = self.call_town(&machine, "colleague_ask", json!(call)).await;
            match (outcome.status.as_str(), outcome.facts.get("id")) {
                ("ok", Some(Value::String(id))) => Ok(Asked { id: id.clone(), machine: machine.clone() }),
                _ => Err(outcome_error(&outcome)),
            }
        };
        let result = match &asked {
            Ok(asked) => asked.id.clone(),
            Err(error) => format!("failed: {}", error.lines().next().unwrap_or_default()),
        };
        self.record_action(requester, &machine, "ask_colleague", label, &result);
        asked
    }

    /// `message_colleague` for a resident of this machine. Words for a resident here go
    /// straight into its session; words for one elsewhere go by mail to its machine. Returns
    /// that machine, and how the words landed when they landed here.
    pub(crate) async fn message_for(&self, from: &str, to: &str, message: &str) -> Result<(String, Option<Spoken>), String> {
        let machine = self.colleague_machine(from, to).await?;
        if machine == self.machine() {
            let spoken = self.speak(&Peer::new(self.machine(), from), to, message).await?;
            self.record_action(from, &machine, "message_colleague", to, super::verbs::landed(spoken));
            return Ok((machine, Some(spoken)));
        }
        let sent = self.mail(&machine, &Letter::Speak { from: from.to_string(), to: to.to_string(), message: message.to_string() });
        let result = match &sent {
            Ok(()) => "sent".to_string(),
            Err(error) => format!("failed: {}", error.lines().next().unwrap_or_default()),
        };
        self.record_action(from, &machine, "message_colleague", to, &result);
        sent.map(|()| (machine, None))
    }

    /// Where a resident someone here wants to speak to lives: this machine, the machine the
    /// speaker asked for it on, or the one machine that has it.
    async fn colleague_machine(&self, speaker: &str, id: &str) -> Result<String, String> {
        if self.lives_here(id) {
            return Ok(self.machine().to_string());
        }
        if let Some((machine, _)) = self.colleagues_of(speaker).into_iter().rev().find(|(_, colleague)| colleague == id) {
            return Ok(machine);
        }
        self.locate(id).await
    }

    /// The machine a resident lives on: this one, else the one other machine whose records
    /// have it. Machines that are offline or did not answer are named when none has it.
    pub(crate) async fn locate(&self, id: &str) -> Result<String, String> {
        if self.lives_here(id) {
            return Ok(self.machine().to_string());
        }
        let query = Query { ids: vec![id.to_string()], ..Query::default() };
        let (answers, silent) = self.ask_others(self.other_members(), &query).await;
        let found: Vec<String> = answers
            .into_iter()
            .filter(|answer| answer.residents.iter().any(|facts| facts.about.id == id && facts.about.machine == answer.machine))
            .map(|answer| answer.machine)
            .collect();
        match found.as_slice() {
            [machine] => Ok(machine.clone()),
            [] if silent.is_empty() => Err(format!("No machine of this World has a resident named {id}.")),
            [] => Err(format!(
                "No machine that answered has a resident named {id}; these did not answer: {}.",
                silent.iter().map(silence).collect::<Vec<_>>().join("; ")
            )),
            several => Err(format!("{id} names residents on several machines: {}.", several.join(", "))),
        }
    }

    /// `fastctx w` across the machines it covers: every answer, this machine's first, and the
    /// machines that gave none with why. A town is asked of its own machines; the World level
    /// and residents named by id, of every machine.
    pub(crate) async fn gather(&self, query: &Query) -> (Vec<MachineAnswer>, Vec<Silent>) {
        let here = crate::town::who::answer(self.world_dir(), self.machine(), query);
        let machines = match (&query.town, query.ids.is_empty() && !query.everywhere) {
            (Some(town), true) => match project::charter(self.world_dir(), town) {
                Ok(charter) => charter.machines.into_iter().filter(|machine| machine != self.machine()).collect(),
                Err(_) => self.other_members(),
            },
            _ => self.other_members(),
        };
        let (mut answers, silent) = self.ask_others(machines, query).await;
        answers.insert(0, here);
        (answers, silent)
    }

    /// Every current member of the World but this machine.
    fn other_members(&self) -> Vec<String> {
        self.world().nodes().into_iter().map(|node| node.name).filter(|name| name != self.machine()).collect()
    }

    /// Asks other machines `who` at once.
    async fn ask_others(&self, machines: Vec<String>, query: &Query) -> (Vec<MachineAnswer>, Vec<Silent>) {
        if machines.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let views = self.world().all_node_views();
        let outcomes = self.world().call_many(machines, "who", json!(query), CALL_TIMEOUT).await;
        let mut answers = Vec::new();
        let mut silent = Vec::new();
        for outcome in outcomes {
            let why = match (outcome.status.as_str(), outcome.facts.get("answer")) {
                ("ok", Some(answer)) => match serde_json::from_value::<MachineAnswer>(answer.clone()) {
                    Ok(answer) => {
                        answers.push(answer);
                        continue;
                    }
                    Err(error) => Silence::NoAnswer(format!("its answer could not be read: {error}")),
                },
                ("offline", _) => Silence::Offline(
                    views.iter().find(|view| view.name == outcome.node).map(|view| view.last_seen.clone()).filter(|seen| !seen.is_empty()),
                ),
                _ => Silence::NoAnswer(outcome_error(&outcome)),
            };
            silent.push(Silent { machine: outcome.node, why });
        }
        (answers, silent)
    }
}

fn silence(silent: &Silent) -> String {
    match &silent.why {
        Silence::Offline(Some(since)) => format!("{} offline since {since}", silent.machine),
        Silence::Offline(None) => format!("{} offline", silent.machine),
        Silence::NoAnswer(reason) => format!("{} ({reason})", silent.machine),
    }
}
