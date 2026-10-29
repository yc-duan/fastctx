//! A stopped turn's quota reading, with Codex's explicit recovery permission.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::agents::{AgentLimits, Fact, LimitWindow};
use fastctx_types::turn_evidence::Quota;
use jiff::Timestamp;
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn read(node: &Arc<Node>, resident: &ResidentId) -> Quota {
    let observed_at = Timestamp::now();
    let result = async {
        let (about, _) = node.residents.about(resident).ok_or_else(|| "unknown conversation".to_string())?;
        if !about.world_managed() {
            return Err(crate::speech::OWN_SESSION.to_string());
        }
        if node.residents.now(resident).is_some_and(|now| now.turn.is_some()) {
            return Err("quota is read for a stopped turn, not while it is running".to_string());
        }
        match about.host.agent.as_str() {
            "codex" => {
                let answer = node.codex.of(node, &about).map_err(|error| error.message)?.rate_limits_now().await?;
                Ok(Quota {
                    observed_at,
                    ordinary_usage_allowed: answer["ordinaryUsageAllowed"].as_bool(),
                    limits: fastctx_hosts::codex::limits_of(&fastctx_hosts::codex::buckets(&answer), observed_at),
                })
            }
            "claude" => {
                let answer = crate::claude::control::usage_now(node, resident).await.map_err(|error| error.message)?;
                Ok(Quota { observed_at, limits: claude(&answer, observed_at)?, ordinary_usage_allowed: None })
            }
            host => Err(format!("no quota reader for {host}")),
        }
    }
    .await;
    result.unwrap_or_else(|reason| Quota { observed_at, limits: Fact::failed(reason), ordinary_usage_allowed: None })
}

fn claude(value: &Value, at: Timestamp) -> Result<Fact<AgentLimits>, String> {
    let rows = value["rateLimits"].as_array().ok_or("Claude did not report rateLimits")?;
    let mut windows = Vec::new();
    for row in rows {
        let name = row["kind"].as_str().ok_or("Claude reported a window without its kind")?.to_string();
        let used = row["percentUsed"].as_f64().ok_or("Claude reported a window without its usage")?;
        if !used.is_finite() || used < 0.0 || used * 10.0 > u32::MAX as f64 {
            return Err("Claude reported an invalid usage percentage".into());
        }
        let resets_at =
            row["resetsAt"].as_str().map(str::parse::<Timestamp>).transpose().map_err(|error| error.to_string())?;
        windows.push(LimitWindow {
            minutes: match name.as_str() {
                "five_hour" => Some(300),
                "seven_day" => Some(10080),
                _ => None,
            },
            name,
            used_tenths: (used * 10.0).round() as u32,
            resets_at,
            since: None,
        });
    }
    if windows.is_empty() {
        return Ok(Fact::unavailable("Claude has not observed usage windows"));
    }
    Ok(Fact::known(AgentLimits { observed_at: at, windows, credits: None }))
}
