//! Catalogs and usage are read from the running host, never inferred from
//! a list of slash commands belonging to another client.

use std::sync::Arc;

use fastctx_types::session::control::{
    CommandArguments, CommandOffer, ContextUsage, ControlKind, Limit, ModelOffer, Offers, ServiceTierOffer, Usage,
};
use serde_json::{Value, json};

use super::controls::Selection;
use super::{CodexHost, Input, Sent, server::AppServer};

pub(super) const COMMANDS: &[(&str, &str, Option<&str>, CommandArguments)] = &[
    ("compact", "Start context compaction between turns", None, CommandArguments::None),
    (
        "model",
        "Select the model and reasoning effort for subsequent World turns",
        Some("[<model> [effort]]"),
        CommandArguments::Optional,
    ),
    ("rename", "Rename this conversation", Some("<name>"), CommandArguments::Required),
    ("fork", "Create a new resident with this conversation's history", None, CommandArguments::None),
    ("review", "Start an inline review", Some("[custom instructions]"), CommandArguments::Optional),
    ("mcp", "List this thread's MCP servers and tools", Some("[verbose]"), CommandArguments::Optional),
    ("skills", "List enabled skills for this conversation", None, CommandArguments::None),
    ("status", "Show this conversation's model, context and account usage", None, CommandArguments::None),
    ("usage", "Show context and account usage", None, CommandArguments::None),
    (
        "goal",
        "Read, set or clear this conversation's goal",
        Some("[get | clear | set <objective>]"),
        CommandArguments::Optional,
    ),
    (
        "tier",
        "Select a catalog service tier for subsequent World turns",
        Some("[<tier-id> | default]"),
        CommandArguments::Optional,
    ),
    ("debug-config", "Read effective configuration and its layers", None, CommandArguments::None),
];

/// Which speed tiers this Codex sends, as its configuration has the modes
/// that govern them (codex-rs `Features::service_tier_enabled`, 0.162): flex
/// always, Ultrafast by `ultrafast_mode`, any other by `fast_mode`. Both are
/// on unless the configuration turns them off.
pub(super) struct Speeds {
    fast: bool,
    ultrafast: bool,
}

impl Speeds {
    pub(super) fn of(config: &Value) -> Self {
        let features = &config["config"]["features"];
        Self { fast: features["fast_mode"] != false, ultrafast: features["ultrafast_mode"] != false }
    }

    pub(super) fn on(&self, tier: &str) -> bool {
        match tier {
            "flex" => true,
            "ultrafast" => self.ultrafast,
            _ => self.fast,
        }
    }

    fn any(&self) -> bool {
        self.fast || self.ultrafast
    }

    /// The mode that turns `tier` off, for saying why it is not taken.
    pub(super) fn mode(tier: &str) -> &'static str {
        if tier == "ultrafast" { "ultrafast_mode" } else { "fast_mode" }
    }
}

impl CodexHost {
    /// The models the app-server lists, kept until it is replaced: the
    /// catalog is read when the conversation is looked at, never for a choice.
    pub(super) async fn models(&self, server: &AppServer) -> Result<Vec<ModelOffer>, String> {
        let models = listed(server).await?;
        self.lock().catalog = Some(models.clone());
        Ok(models)
    }

    /// The catalog last listed, if any; a choice made before any listing is
    /// checked by Codex itself when the next turn starts.
    pub(super) fn catalog(&self) -> Option<Vec<ModelOffer>> {
        self.lock().catalog.clone()
    }

    async fn skills(&self, server: &AppServer, thread: &str) -> Result<Vec<Value>, String> {
        let cwd = self
            .lock()
            .threads
            .get(thread)
            .and_then(|entry| entry.spec.as_ref())
            .map(|spec| spec.cwd.clone())
            .ok_or("this thread's working directory is not known")?;
        // Codex keeps its skill catalog and says when it changes
        // (`skills/changed`), so it is not read from disk again here.
        let answer = server.request("skills/list", json!({"cwds":[cwd],"forceReload":false})).await?;
        let entries = answer["data"].as_array().ok_or("skills/list answered without data")?;
        let mut skills = Vec::new();
        for entry in entries {
            if let Some(errors) = entry["errors"].as_array().filter(|errors| !errors.is_empty()) {
                return Err(format!("Codex could not read its skill catalog: {}", json!(errors)));
            }
            for skill in entry["skills"].as_array().ok_or("skills/list answered without skills")? {
                if skill["enabled"] == true {
                    skills.push(skill.clone());
                }
            }
        }
        Ok(skills)
    }

    pub async fn offers(self: &Arc<Self>, thread: &str) -> Result<Offers, String> {
        let server = self.server().await?;
        // Inspecting a sleeping resident must not subscribe it again.
        // Catalogs are server-wide; unavailable per-thread usage is reported.
        let speeds = Speeds::of(&self.read_config(thread).await?);
        let mut models = self.models(&server).await?;
        // A tier whose mode is off is one Codex leaves out of its requests.
        for model in &mut models {
            model.service_tiers.retain(|tier| speeds.on(&tier.id));
            if model.default_service_tier.as_deref().is_some_and(|tier| !speeds.on(tier)) {
                model.default_service_tier = None;
            }
        }
        let skills = self.skills(&server, thread).await?;
        // The account's limits and usage are not read here: reading them asks
        // OpenAI, which only the user's own asking does (`usage`); the limits
        // Codex pushes while it works are what is shown.
        let mut commands: Vec<CommandOffer> = COMMANDS
            .iter()
            .map(|(name, description, args, arguments)| CommandOffer {
                name: (*name).into(),
                description: (*description).into(),
                args: args.map(str::to_string),
                argument_forms: Vec::new(),
                arguments: *arguments,
                source: Some("codex".into()),
                path: None,
            })
            .collect();
        if !speeds.any() {
            commands.retain(|command| command.name != "tier");
        }
        // Goals are not offered in a release yet (`world.md`, 这一版不发的).
        if !fastctx_platform::preview() {
            commands.retain(|command| command.name != "goal");
        }
        for skill in skills {
            let name = string(&skill, "name")?;
            commands.push(CommandOffer {
                name,
                description: string(&skill, "description")?,
                args: Some("[instructions]".into()),
                argument_forms: Vec::new(),
                arguments: CommandArguments::Optional,
                source: Some("skill".into()),
                path: Some(string(&skill, "path")?),
            });
        }
        // A thread not loaded since this app-server started knows the
        // choices made for it only where they were kept: it offers them as
        // a loaded one does, whatever its session is doing.
        let kept = {
            let mut state = self.lock();
            let entry = state.threads.get(thread).ok_or("unknown Codex thread")?;
            entry.spec.as_ref().filter(|_| !entry.loaded).map(|spec| spec.choices.clone())
        };
        let kept = kept.map(|choices| Selection::read(&choices)).transpose()?;
        // A thread not loaded since this app-server started has not said how
        // it keeps its history, which decides whether it can go back; Codex
        // says it without loading the thread.
        let unknown = self.lock().threads.get(thread).is_some_and(|entry| entry.metadata["historyMode"].is_null());
        if unknown
            && let Ok(answer) = server.ask("thread/read", json!({"threadId": thread, "includeTurns": false})).await
        {
            let mut state = self.lock();
            if let Some(entry) = state.threads.get_mut(thread).filter(|entry| entry.metadata["historyMode"].is_null()) {
                entry.metadata = super::controls::metadata(&answer["thread"]);
            }
        }
        let mut state = self.lock();
        let entry = state.threads.get(thread).ok_or("unknown Codex thread")?;
        let chosen = kept.as_ref().unwrap_or(&entry.selection);
        // What it was started with stands until Codex or a choice says more.
        let started = |key: &str| entry.spec.as_ref().and_then(|spec| spec.started(key));
        let model = chosen.model.clone().or_else(|| entry.model.clone()).or_else(|| started("model"));
        let effort =
            chosen.effort.clone().or_else(|| entry.effort.clone()).or_else(|| started("model_reasoning_effort"));
        let service_tier = chosen.service_tier.clone().or_else(|| entry.service_tier.clone());
        if let Some(found) = models.iter().find(|offer| Some(&offer.id) == model.as_ref()) {
            for tier in &found.service_tiers {
                let name = tier.name.to_lowercase();
                // A tier name becomes a slash command only when it is a word
                // and cannot shadow another host command or a named skill.
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && !commands.iter().any(|command| command.name == name)
                {
                    commands.push(CommandOffer {
                        name,
                        description: tier.description.clone(),
                        args: None,
                        argument_forms: Vec::new(),
                        arguments: CommandArguments::None,
                        source: Some("codex".into()),
                        path: None,
                    });
                }
            }
        }
        // What it can do at all, not just between turns: a compaction asked
        // for while a turn runs is refused then, and the interface waits.
        let mut controls = vec![
            ControlKind::Command,
            ControlKind::Model,
            ControlKind::Rename,
            ControlKind::Fork,
            ControlKind::Compact,
        ];
        if entry.metadata["historyMode"] == "paginated" && entry.metadata["ephemeral"] != true {
            controls.push(ControlKind::Rewind);
        }
        if entry.requests.values().any(|request| request.supported()) {
            controls.push(ControlKind::Answer);
        }
        let context = entry.usage.as_ref().and_then(|usage| {
            Some(ContextUsage {
                used: usage.pointer("/last/totalTokens")?.as_u64()?,
                window: usage["modelContextWindow"].as_u64(),
            })
        });
        let mut usage = Usage { context, ..Usage::default() };
        // Thread usage is explicitly an estimate in 0.160. Preserve its
        // credits/USD-micros contract instead of calling it a final cost.
        usage.raw = json!({"tokenUsage":entry.usage});
        Ok(Offers {
            pending_controls: Vec::new(),
            control_queue: false,
            controls_told: false,
            control_reply_envelope: false,
            checkpoints: None,
            history_limits: None,
            automatic_model: None,
            controls,
            commands,
            models,
            model,
            effort,
            service_tier,
            usage: Some(usage),
        })
    }

    /// The account's limits and usage as OpenAI says them now: asked only
    /// when the user asks (`/usage`, `/status`).
    pub async fn read_usage(self: &Arc<Self>, thread: &str) -> Result<Usage, String> {
        let server = self.server().await?;
        let read_at = jiff::Timestamp::now();
        let limits = server.request("account/rateLimits/read", json!({})).await?;
        self.limits_heard(read_at, super::account::LimitsHow::Read(limits.clone()));
        let account_usage = match server.request("account/usage/read", json!({"threadId":thread})).await {
            Ok(usage) => usage,
            Err(error) => json!({"error":error}),
        };
        let mut usage = Usage { limits: limits_of(&limits)?, limits_at: Some(read_at), ..Usage::default() };
        usage.raw = json!({"accountRateLimits":limits,"accountUsage":account_usage});
        Ok(usage)
    }

    pub async fn skill(
        self: &Arc<Self>,
        thread: &str,
        name: &str,
        path: Option<&str>,
        args: &str,
    ) -> Result<Sent, String> {
        self.prepare(thread).await?;
        let server = self.server().await?;
        let skills = self.skills(&server, thread).await?;
        let matches: Vec<_> = skills
            .iter()
            .filter(|skill| {
                skill["name"].as_str() == Some(name) && path.is_none_or(|path| skill["path"].as_str() == Some(path))
            })
            .collect();
        let skill = match matches.as_slice() {
            [skill] => skill,
            [] => {
                return Err(format!(
                    "the World cannot execute /{name}: it is neither an implemented command nor an enabled skill"
                ));
            }
            _ => return Err(format!("more than one enabled skill is named {name}")),
        };
        let text = format!("${name} {args}").trim_end().to_string();
        self.send(thread, Input::Skill { name: name.into(), path: string(skill, "path")?, text }, None)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn mcp(self: &Arc<Self>, thread: &str) -> Result<Value, String> {
        self.prepare(thread).await?;
        let server = self.server().await?;
        pages(&server, "mcpServerStatus/list", json!({"threadId":thread,"detail":"full"})).await.map(|data| json!(data))
    }

    pub async fn review(
        self: &Arc<Self>,
        thread: &str,
        args: &str,
    ) -> Result<Value, super::control_error::ControlError> {
        self.prepare(thread).await?;
        if self.running_turn(thread).is_some() {
            return Err("start a review between turns".into());
        }
        let target = if args.is_empty() {
            json!({"type":"uncommittedChanges"})
        } else {
            json!({"type":"custom","instructions":args})
        };
        self.start_control(thread, "review/start", json!({"threadId":thread,"target":target,"delivery":"inline"})).await
    }
}

async fn listed(server: &AppServer) -> Result<Vec<ModelOffer>, String> {
    let entries = pages(server, "model/list", json!({"includeHidden":false})).await?;
    entries
        .iter()
        .map(|model| {
            Ok(ModelOffer {
                id: string(model, "model")?,
                name: model["displayName"].as_str().map(str::to_string),
                description: model["description"].as_str().map(str::to_string),
                efforts: model["supportedReasoningEfforts"]
                    .as_array()
                    .ok_or("model has no supportedReasoningEfforts")?
                    .iter()
                    .map(|effort| string(effort, "reasoningEffort"))
                    .collect::<Result<_, _>>()?,
                default_effort: model["defaultReasoningEffort"].as_str().map(str::to_string),
                default: model["isDefault"].as_bool().ok_or("model has no isDefault")?,
                service_tiers: match model.get("serviceTiers") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()
                        .ok_or("model has no serviceTiers array")?
                        .iter()
                        .map(|tier| {
                            Ok(ServiceTierOffer {
                                id: string(tier, "id")?,
                                name: string(tier, "name")?,
                                description: string(tier, "description")?,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                },
                default_service_tier: model["defaultServiceTier"].as_str().map(str::to_string),
                context_window: None,
            })
        })
        .collect()
}

/// Every window of an `account/rateLimits/read` answer, each bucket named.
fn limits_of(answer: &Value) -> Result<Vec<Limit>, String> {
    let snapshots: Vec<(&str, &Value)> = match answer["rateLimitsByLimitId"].as_object() {
        Some(buckets) if !buckets.is_empty() => buckets.iter().map(|(name, value)| (name.as_str(), value)).collect(),
        _ => vec![("codex", &answer["rateLimits"])],
    };
    let mut limits = Vec::new();
    for (bucket, snapshot) in snapshots {
        for kind in ["primary", "secondary"] {
            let window = &snapshot[kind];
            if window.is_null() {
                continue;
            }
            let duration = window["windowDurationMins"].as_u64().map(|mins| format!(" {mins}m")).unwrap_or_default();
            let reset = window["resetsAt"]
                .as_i64()
                .map(jiff::Timestamp::from_second)
                .transpose()
                .map_err(|error| format!("Codex reported an invalid rate-limit reset: {error}"))?;
            limits.push(Limit {
                name: format!("{bucket} {kind}{duration}"),
                used_percent: window["usedPercent"].as_number().cloned(),
                resets_at: reset,
            });
        }
    }
    Ok(limits)
}

fn string(value: &Value, field: &str) -> Result<String, String> {
    value[field].as_str().map(str::to_string).ok_or_else(|| format!("Codex catalog entry has no string {field}"))
}

async fn pages(server: &AppServer, method: &str, mut params: Value) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    loop {
        let answer = server.request(method, params.clone()).await?;
        values.extend(
            answer["data"].as_array().ok_or_else(|| format!("{method} answered without data"))?.iter().cloned(),
        );
        match answer["nextCursor"].as_str() {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => return Ok(values),
        }
    }
}
