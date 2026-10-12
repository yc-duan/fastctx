//! Catalogs and usage are read from the running host, never inferred from
//! a list of slash commands belonging to another client.

use std::sync::Arc;

use fastctx_types::session::control::{
    CommandArguments, CommandOffer, ContextUsage, ControlKind, Limit, ModelOffer, Offers, ServiceTierOffer, Usage,
};
use serde_json::{Value, json};

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

impl CodexHost {
    pub(super) async fn models(&self, server: &AppServer) -> Result<Vec<ModelOffer>, String> {
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

    async fn skills(&self, server: &AppServer, thread: &str) -> Result<Vec<Value>, String> {
        let cwd = self
            .lock()
            .threads
            .get(thread)
            .and_then(|entry| entry.spec.as_ref())
            .map(|spec| spec.cwd.clone())
            .ok_or("this thread's working directory is not known")?;
        let answer = server.request("skills/list", json!({"cwds":[cwd],"forceReload":true})).await?;
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
        let models = self.models(&server).await?;
        let fast_mode = self.read_config(thread).await?["config"]["features"]["fast_mode"] != false;
        let skills = self.skills(&server, thread).await?;
        let read_at = jiff::Timestamp::now();
        let limits = server.request("account/rateLimits/read", json!({})).await;
        if let Ok(answer) = &limits {
            self.limits_heard(read_at, super::account::LimitsHow::Read(answer.clone()));
        }
        let account_usage = match server.request("account/usage/read", json!({"threadId":thread})).await {
            Ok(usage) => usage,
            Err(error) => json!({"error":error}),
        };
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
        if !fast_mode {
            commands.retain(|command| command.name != "tier");
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
        let mut state = self.lock();
        let entry = state.threads.get(thread).ok_or("unknown Codex thread")?;
        let selected_model = entry.selection.model.as_ref().or(entry.model.as_ref());
        if fast_mode && let Some(model) = models.iter().find(|model| Some(&model.id) == selected_model) {
            for tier in &model.service_tiers {
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
        let mut controls = vec![ControlKind::Command, ControlKind::Model, ControlKind::Rename, ControlKind::Fork];
        if entry.turn.is_none() && !entry.submitted {
            controls.push(ControlKind::Compact);
        } else {
            commands.retain(|command| command.path.is_some() || !matches!(command.name.as_str(), "compact" | "review"));
        }
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
        let account = match limits {
            Ok(answer) => {
                let snapshots: Vec<(&str, &Value)> = match answer["rateLimitsByLimitId"].as_object() {
                    Some(buckets) if !buckets.is_empty() => {
                        buckets.iter().map(|(name, value)| (name.as_str(), value)).collect()
                    }
                    _ => vec![("codex", &answer["rateLimits"])],
                };
                for (bucket, snapshot) in snapshots {
                    for kind in ["primary", "secondary"] {
                        let window = &snapshot[kind];
                        if window.is_null() {
                            continue;
                        }
                        let duration =
                            window["windowDurationMins"].as_u64().map(|mins| format!(" {mins}m")).unwrap_or_default();
                        let reset = window["resetsAt"]
                            .as_i64()
                            .map(jiff::Timestamp::from_second)
                            .transpose()
                            .map_err(|error| format!("Codex reported an invalid rate-limit reset: {error}"))?;
                        usage.limits.push(Limit {
                            name: format!("{bucket} {kind}{duration}"),
                            used_percent: window["usedPercent"].as_number().cloned(),
                            resets_at: reset,
                        });
                    }
                }
                answer
            }
            Err(error) => json!({"error":error}),
        };
        // Thread usage is explicitly an estimate in 0.160. Preserve its
        // credits/USD-micros contract instead of calling it a final cost.
        usage.raw = json!({"tokenUsage":entry.usage,"accountRateLimits":account,"accountUsage":account_usage});
        Ok(Offers {
            checkpoints: None,
            history_limits: None,
            automatic_model: None,
            controls,
            commands,
            models,
            model: entry.selection.model.clone().or_else(|| entry.model.clone()),
            effort: entry.selection.effort.clone().or_else(|| entry.effort.clone()),
            service_tier: entry.selection.service_tier.clone().or_else(|| entry.service_tier.clone()),
            usage: Some(usage),
        })
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
        self.send(thread, Input::Skill { name: name.into(), path: string(skill, "path")?, text })
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn mcp(self: &Arc<Self>, thread: &str) -> Result<Value, String> {
        self.prepare(thread).await?;
        let server = self.server().await?;
        pages(&server, "mcpServerStatus/list", json!({"threadId":thread,"detail":"full"})).await.map(|data| json!(data))
    }

    pub async fn review(self: &Arc<Self>, thread: &str, args: &str) -> Result<Value, String> {
        self.prepare(thread).await?;
        if self.running_turn(thread).is_some() {
            return Err("start a review between turns".into());
        }
        let server = self.server().await?;
        let target = if args.is_empty() {
            json!({"type":"uncommittedChanges"})
        } else {
            json!({"type":"custom","instructions":args})
        };
        server.request("review/start", json!({"threadId":thread,"target":target,"delivery":"inline"})).await
    }
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
