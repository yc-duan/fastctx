//! One wake, one bounded conversation, one atomic conclusion.
use crate::{
    ports::{Look, Machine, ReadResult, Web},
    state::{State, Store},
    words,
};
use fastctx_agent::{
    Application, Decision, FutureResult, Limits, ToolOutput,
    model::{Model, Tool, ToolCall},
    runtime::bounded,
};
use fastctx_types::sentinel::*;
use serde::Deserialize;
use serde_json::{Value, json};

/// Capabilities fixed by the host for this wake, never supplied by the model.
pub struct Environment<'a> {
    pub model: &'a dyn Model,
    pub look: &'a dyn Look,
    pub web: &'a dyn Web,
    /// The machines it may look at this wake.
    pub machines: &'a [Machine],
    pub limits: &'a Limits,
}

/// Runs under the caller's per-sentinel gate. `wake_id` is stable across retries.
pub async fn wake(
    store: &Store,
    mut state: State,
    wake_id: &str,
    mut reason: WakeReason,
    now: i64,
    environment: &Environment<'_>,
) -> Result<State, String> {
    let Environment { model, look, web, machines, limits } = *environment;
    if state.wake_id == wake_id || state.finished {
        if state.pending.as_ref().is_some_and(|pending|pending.id==wake_id) {
            state.pending=None;
            store.checkpoint(&state)?;
        }
        return Ok(state);
    }
    state.sequence += 1;
    state.pending = Some(crate::state::WakeIntent { id: wake_id.into(), reason: reason.clone(), at: now });
    store.checkpoint(&state)?;
    if matches!(reason, WakeReason::Withdrawn) {
        state.conclusion = Conclusion::Withdrawn;
        state.finished = true;
        state.completed_at = now;
        state.reflexes.clear();
        state.wake_id = wake_id.into();
        state.pending = None;
        store.commit(&state)?;
        return Ok(state);
    }
    let mut observations = Vec::new();
    if let WakeReason::Reflex { observation, .. } = &mut reason {
        if observation.text.len() > 65_536 { return Err("reflex evidence is too large".into()); }
        if observation.full_path.is_none() {
            let path = store.directory().join("evidence").join(format!("reflex-{}.txt",state.sequence));
            std::fs::create_dir_all(path.parent().expect("evidence directory")).map_err(|e|e.to_string())?;
            fastctx_platform::publish(&path,observation.text.as_bytes(),fastctx_platform::Permissions::Private).map_err(|e|e.to_string())?;
            observation.full_path=Some(path.to_string_lossy().into_owned());
        }
        observation.text=bounded(&observation.text,8192);
        observations.push(observation.clone());
        if state.definition.kind==Kind::Web { observation.full_path=None; }
    }
    let recent: Vec<_> = state.recent.iter().map(|o| json!({"id":o.id,"at":o.at,"source":bounded(&o.source,256),"text":bounded(&o.text,1024),"full_path":if state.definition.kind==Kind::Web { None } else { o.full_path.as_ref() }})).collect();
    let reflexes: Vec<_> = state.reflexes.iter().map(|r| json!({"id":r.id,"at":r.at,"every_ms":r.every_ms})).collect();
    let input = if let WakeReason::Goal { review } = &reason { crate::goal::input(review)?.to_string() } else { json!({"condition":state.definition.condition,"kind":state.definition.kind,"at":state.definition.at,"plan_claim":state.plan,"notes_claim":state.notes,"recent":recent,
        "reflexes":reflexes,"machines":machines,"wake":reason,"now":now}).to_string() };
    let goal = state.definition.kind == Kind::Goal;
    if goal != matches!(reason, WakeReason::Goal { .. }) {
        return Err("goal wake and sentinel kind disagree".into());
    }
    let mut app = Sentinel {
        state,
        now,
        machines,
        look,
        web,
        directory: store.directory().to_path_buf(),
        observations,
        missing_wake: false,
        obstacle_recorded: false,
        invalid_reports: 0,
    };
    let system = if goal { words::GOAL } else if app.state.definition.kind == Kind::Web { words::WATCH_WEB } else { words::WATCH };
    let path = store.run_path(app.state.sequence);
    let run =
        fastctx_agent::run(model, &mut app, system.into(), input, limits, &path)
            .await;
    let run = run?;
    if let Some(value) = run.result {
        app.state.conclusion = serde_json::from_value(value).map_err(|e| e.to_string())?;
    } else {
        app.state.conclusion = Conclusion::NotJudged { reason: run.error.unwrap_or_else(|| "no conclusion".into()) };
    }
    app.state.usage = run.usage;
    app.state.requests = run.requests;
    app.state.unreported_requests = run.unreported_requests;
    app.state.reserved_tokens = run.reserved_tokens;
    app.state.elapsed_ms = run.elapsed_ms;
    app.state.completed_at = now.saturating_add(run.elapsed_ms.min(i64::MAX as u64) as i64);
    app.state.finished = matches!(
        app.state.conclusion,
        Conclusion::Satisfied { .. } | Conclusion::Unable { .. } | Conclusion::Withdrawn
    );
    if app.state.finished {
        app.state.reflexes.clear();
    }
    if !goal && !app.state.finished && !app.state.reflexes.iter().any(|r| r.at > now && r.at < i64::MAX) {
        app.state.reflexes.push(retry_reflex(now));
    }
    app.state.recent.extend(app.observations);
    let excess = app.state.recent.len().saturating_sub(8);
    app.state.recent.drain(..excess);
    app.state.wake_id = wake_id.into();
    app.state.pending = None;
    store.commit(&app.state)?;
    Ok(app.state)
}

pub(crate) fn retry_reflex(now: i64) -> Reflex {
    Reflex {
        id: "retry".into(),
        at: now.saturating_add(300_000),
        every_ms: None,
        probe: None,
        predicate: None,
        last_fingerprint: None,
    }
}
struct Sentinel<'a> {
    state: State,
    now: i64,
    machines: &'a [Machine],
    look: &'a dyn Look,
    web: &'a dyn Web,
    directory: std::path::PathBuf,
    observations: Vec<Observation>,
    missing_wake: bool,
    obstacle_recorded: bool,
    invalid_reports: u8,
}
fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}
fn string() -> Value {
    json!({"type":"string"})
}
impl Sentinel<'_> {
    /// A machine parameter that names one of the machines it may look at.
    fn machine(&self) -> Value {
        if self.machines.is_empty() {
            return string();
        }
        let names: Vec<_> = self.machines.iter().map(|machine| machine.name.as_str()).collect();
        json!({"type":"string","enum":names})
    }
}
impl Application for Sentinel<'_> {
    fn tools(&self) -> Vec<Tool> {
        let mut tools = match self.state.definition.kind {
            Kind::Local | Kind::Goal => {
                vec![tool("look", words::LOOK, json!({"machine":self.machine(),"command":string()}), &["machine", "command"])]
            }
            Kind::Web => vec![
                tool("fetch_web", words::FETCH, json!({"url":string()}), &["url"]),
                tool("search", words::SEARCH, json!({"query":string()}), &["query"]),
            ],
        };
        if self.state.definition.kind != Kind::Goal {
            let probe = if self.state.definition.kind == Kind::Web {
                json!({"type":"object","properties":{"kind":{"const":"web"},"url":string()},"required":["kind","url"],"additionalProperties":false})
            } else {
                json!({"type":"object","properties":{"kind":{"const":"look"},"machine":self.machine(),"command":string()},"required":["kind","machine","command"],"additionalProperties":false})
            };
            tools.push(tool("install_reflex",words::REFLEX, json!({
                "id":string(),"at":{"type":"integer"},"every_ms":{"type":["integer","null"]},
                "probe":{"oneOf":[{"type":"null"},probe]},
                "predicate":{"oneOf":[{"type":"null"},{"type":"object","properties":{"kind":{"enum":["contains","number","changed","exit_nonzero"]},"text":string(),"prefix":string(),"comparison":{"enum":["less","less_equal","greater","greater_equal","equal"]},"value":{"type":"number"}},"required":["kind"]}]}
            }), &["id","at"]));
            tools.push(tool("report",words::REPORT,json!({"status":{"enum":["watching","satisfied","unable"]},"reason":string(),"obstacle":string(),"evidence":{"type":"array","items":string()},"plan":string(),"notes":string()}), &["status"]));
        } else {
            tools.push(tool("report",words::GOAL_REPORT,json!({"sound":{"type":"boolean"},"reason":string(),"looked":string(),"saw":string(),"thinks":string()}), &["sound"]));
        }
        tools
    }
    fn call<'a>(&'a mut self, call: &'a ToolCall) -> FutureResult<'a, ToolOutput> {
        Box::pin(async move {
            let a = &call.arguments;
            match call.name.as_str() {
                "look" if self.state.definition.kind != Kind::Web => {
                    let machine =
                        required(a, "machine")?.parse().map_err(|e: fastctx_types::NameError| e.to_string())?;
                    let command = required(a, "command")?;
                    let result = self.look.look(&self.state.definition, &machine, command).await?;
                    self.observe(format!("{machine}: {command}"), result)
                }
                "fetch_web" if self.state.definition.kind == Kind::Web => {
                    let url = required(a, "url")?;
                    let result = self.web.fetch(url).await?;
                    self.observe(url.into(), result)
                }
                "search" if self.state.definition.kind == Kind::Web => {
                    let query = required(a, "query")?;
                    let result = self.web.search(query).await?;
                    self.observe(format!("search: {query}"), result)
                }
                "install_reflex" if self.state.definition.kind != Kind::Goal => {
                    let mut reflex: Reflex = serde_json::from_value(a.clone()).map_err(|e| e.to_string())?;
                    reflex.last_fingerprint = None;
                    crate::reflex::validate(&reflex, self.state.definition.kind, self.now)?;
                    if let Some(index) = self.state.reflexes.iter().position(|r| r.id == reflex.id) {
                        self.state.reflexes[index] = reflex;
                    } else if self.state.reflexes.len() < 32 {
                        self.state.reflexes.push(reflex);
                    } else {
                        return Err("reflex limit exceeded".into());
                    }
                    Ok(ToolOutput::value(json!({"installed":true})))
                }
                "report" => match self.checked_conclusion(Ok(a.clone()))? {
                    Decision::Finish(value) => Ok(ToolOutput { value: json!({"accepted":true}), finish: Some(value) }),
                    Decision::Continue(reason) => Err(reason),
                },
                _ => Err(fastctx_agent::words::UNKNOWN_TOOL.into()),
            }
        })
    }
    fn finish(&mut self, text: &str) -> Result<Decision, String> {
        self.checked_conclusion(serde_json::from_str::<Value>(text).map_err(|e| e.to_string()))
    }
}
impl Sentinel<'_> {
    fn checked_conclusion(&mut self, value: Result<Value, String>) -> Result<Decision, String> {
        match value.and_then(|value| self.conclude(&value)) {
            Err(error) => {
                self.invalid_reports += 1;
                if self.invalid_reports >= 3 { done(Conclusion::NotJudged { reason: error }) } else { Err(error) }
            }
            result => result,
        }
    }
    fn observe(&mut self, source: String, result: ReadResult) -> Result<ToolOutput, String> {
        if result.text.len() > 65_536 {
            return Err("observation exceeded 65536 bytes".into());
        }
        let id = format!("{}-{}", self.state.sequence, self.observations.len() + 1);
        let full_path = match result.full_path {
            Some(path) => path,
            None => {
                let path = self.directory.join("evidence").join(format!("{id}.txt"));
                std::fs::create_dir_all(path.parent().expect("evidence directory")).map_err(|e| e.to_string())?;
                fastctx_platform::publish(&path, result.text.as_bytes(), fastctx_platform::Permissions::Private)
                    .map_err(|e| e.to_string())?;
                path.to_string_lossy().into_owned()
            }
        };
        let observation = Observation {
            id,
            at: self.now,
            source: bounded(&source, 8192),
            text: bounded(&result.text, 8192),
            exit_code: result.exit_code,
            full_path: Some(full_path),
        };
        let mut output = serde_json::to_value(&observation).map_err(|e| e.to_string())?;
        if self.state.definition.kind == Kind::Web {
            output.as_object_mut().expect("observation object").remove("full_path");
        }
        self.observations.push(observation);
        Ok(ToolOutput::value(output))
    }
    fn conclude(&mut self, value: &Value) -> Result<Decision, String> {
        if self.state.definition.kind == Kind::Goal {
            let sound = value["sound"].as_bool().ok_or("missing sound")?;
            let judgment = if sound {
                Judgment::Sound { reason: required(value, "reason")?.into() }
            } else {
                Judgment::Advice {
                    looked: required(value, "looked")?.into(),
                    saw: required(value, "saw")?.into(),
                    thinks: required(value, "thinks")?.into(),
                }
            };
            return done(Conclusion::Goal { judgment });
        }
        let report: Report = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if let Some(plan) = report.plan {
            self.state.plan = bounded(&plan, 4096);
        }
        if let Some(notes) = report.notes {
            self.state.notes = bounded(&notes, 4096);
        }
        match report.status.as_str() {
            "satisfied" => {
                if report.evidence.is_empty()
                    || report.evidence.iter().any(|id| !self.observations.iter().any(|o| &o.id == id))
                {
                    return Ok(Decision::Continue(words::NEED_EVIDENCE.into()));
                }
                let reason = required(value, "reason")?.to_string();
                let evidence = self.observations.iter().filter(|o| report.evidence.contains(&o.id)).cloned().collect();
                done(Conclusion::Satisfied { reason, evidence })
            }
            "unable" => {
                let obstacle = required(value, "obstacle")?;
                if !self.obstacle_recorded {
                    if self.state.obstacle.as_deref() == Some(obstacle) {
                        self.state.obstacle_count += 1;
                    } else {
                        self.state.obstacle = Some(obstacle.into());
                        self.state.obstacle_count = 1;
                    }
                    self.obstacle_recorded = true;
                }
                if self.state.obstacle_count >= 3 {
                    done(Conclusion::Unable { reason: required(value, "reason")?.into() })
                } else {
                    self.state.reflexes.retain(|r| r.id != "retry");
                    self.state.reflexes.push(retry_reflex(self.now));
                    done(Conclusion::Watching)
                }
            }
            "watching" => {
                self.state.obstacle = None;
                self.state.obstacle_count = 0;
                if self.state.reflexes.iter().any(|r| r.at > self.now && r.at < i64::MAX) {
                    done(Conclusion::Watching)
                } else if self.missing_wake {
                    done(Conclusion::Unable { reason: words::NO_WAKE.into() })
                } else {
                    self.missing_wake = true;
                    Ok(Decision::Continue(words::NEED_WAKE.into()))
                }
            }
            _ => Err("invalid report status".into()),
        }
    }
}
fn done(conclusion: Conclusion) -> Result<Decision, String> {
    serde_json::to_value(conclusion).map(Decision::Finish).map_err(|e| e.to_string())
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 8192)
        .ok_or_else(|| format!("missing or oversized {key}"))
}
#[derive(Deserialize)]
struct Report {
    status: String,
    #[serde(default)]
    evidence: Vec<String>,
    plan: Option<String>,
    notes: Option<String>,
}
