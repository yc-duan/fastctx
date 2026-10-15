//! This computer's agents on the machines page, a line for each of their
//! logins (`world-agent-logins.md`): who is logged in and on what plan, how
//! much of each usage window is spent and when that was read, and whether
//! sessions the user opens in it have the World's tools, and what bringing
//! them up to date waits for. The line the keys are on is the login `u` and
//! `x` put the tools into or take them out of, and `t` takes them back in.

use std::path::PathBuf;

use fastctx_types::agents::{AgentFacts, AgentsReply, Fact, HomeFound, Integration, LimitWindow, LoginStatus};
use fastctx_types::logins::Drift;
use ratatui::text::{Line, Span};

use super::{quiet, title, wrong};
use crate::app::Ctx;
use crate::design::Role;
use crate::paint::Row;
use crate::sessions::Host;
use crate::text;

/// A login the keys can act on: which agent, its directory, and how it
/// stands now.
pub struct Login {
    pub host: Host,
    pub home: PathBuf,
    pub standing: Option<Integration>,
    /// What bringing its tools up to date waits for, if anything.
    pub drift: Option<Drift>,
}

/// The logins, in the order they are drawn.
pub fn logins(reply: &AgentsReply) -> Vec<Login> {
    ordered(reply)
        .into_iter()
        .filter_map(|facts| {
            let host = match facts.agent.as_str() {
                "codex" => Host::Codex,
                "claude" => Host::Claude,
                _ => return None,
            };
            let home = facts.home.as_ref()?.dir.clone();
            let standing = match &facts.integration {
                Fact::Known { value } => Some(*value),
                _ => None,
            };
            Some(Login { host, home, standing, drift: facts.drift.clone() })
        })
        .collect()
}

/// Codex first, then Claude Code, each agent's default login first; a
/// login whose program is missing is not one the keys act on.
fn ordered(reply: &AgentsReply) -> Vec<&AgentFacts> {
    let mut all: Vec<&AgentFacts> =
        reply.agents.iter().filter(|facts| facts.home.is_some() && installed(facts)).collect();
    let rank = |facts: &AgentFacts| {
        let agent = if facts.agent.as_str() == "codex" { 0 } else { 1 };
        let found = facts.home.as_ref().map_or(0, |home| if home.found == HomeFound::Default { 0 } else { 1 });
        (agent, found)
    };
    all.sort_by_key(|facts| rank(facts));
    all
}

fn installed(facts: &AgentFacts) -> bool {
    matches!(&facts.program, Fact::Known { value: Some(program) } if program.exists)
}

/// The rows for this computer's agents; each login is an item, numbered on
/// from `item`.
pub fn rows(ctx: &Ctx, reply: &AgentsReply, item: &mut usize, width: usize) -> Vec<Row> {
    let words = &ctx.words;
    let look = &ctx.look;
    let mut rows = Vec::new();
    for agent in ["codex", "claude"] {
        let name = if agent == "codex" { "Codex" } else { "Claude Code" };
        let all: Vec<&AgentFacts> = reply.agents.iter().filter(|facts| facts.agent.as_str() == agent).collect();
        if all.is_empty() || !all.iter().any(|facts| installed(facts)) {
            rows.push(Row::new(Line::from(vec![
                Span::styled(name, look.text(Role::Muted)),
                Span::styled(format!("  {}", words.agent_not_installed()), look.text(Role::Faint)),
            ])));
            continue;
        }
        let many = all.len() > 1;
        for facts in ordered(reply).into_iter().filter(|facts| facts.agent.as_str() == agent) {
            rows.extend(login_rows(ctx, name, facts, many, *item, width));
            *item += 1;
        }
    }
    rows
}

/// One login: who and on what plan, with its usage on the right; below it,
/// where it is kept when there are several, whether the World's tools are
/// in, and when the usage was read.
fn login_rows(ctx: &Ctx, name: &str, facts: &AgentFacts, many: bool, item: usize, width: usize) -> Vec<Row> {
    let words = &ctx.words;
    let look = &ctx.look;
    let mut left = vec![title(ctx, name.to_string()), Span::raw("  ")];
    match &facts.login {
        Fact::Known { value: LoginStatus::LoggedOut } => {
            left.push(Span::styled(words.agent_logged_out(), look.text(Role::Caution)));
        }
        login => {
            let account = match &facts.account {
                Fact::Known { value } => Some(value),
                _ => None,
            };
            let who = account.and_then(|account| account.email.clone()).unwrap_or_else(|| match login {
                Fact::Known { .. } => words.agent_logged_in().to_string(),
                _ => words.agent_login_unknown().to_string(),
            });
            left.push(Span::styled(who, look.text(Role::Text)));
            if let Some(plan) = account.and_then(|account| account.plan.as_deref()) {
                left.push(Span::styled(format!("  {}", plan_name(plan)), look.text(Role::Muted)));
            }
        }
    }
    let (limits, read) = match &facts.limits {
        Fact::Known { value } if !value.windows.is_empty() => {
            let mut read = words.read_ago(&words.ago(value.observed_at, ctx.now));
            if let Some(credits) = &value.credits {
                read = format!("{}  ·  {read}", words.credits(credits.balance.as_deref(), credits.unlimited));
            }
            (limits_line(ctx, &value.windows), Some(read))
        }
        Fact::Failed { reason } => (wrong(ctx, words.limits_failed(&text::first_line(reason))), None),
        _ => (Line::default(), None),
    };
    let mut rows = vec![Row::new(Line::from(left)).right(limits).item(item)];
    let mut said = Vec::new();
    if let Some(home) = &facts.home
        && (many || home.found != HomeFound::Default)
    {
        let dir = home.dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        said.push(format!("{dir} · {}", words.login_found(home.found)));
    }
    if let Fact::Known { value } = &facts.integration
        && *value != Integration::Absent
    {
        said.push(words.login_tools(*value).to_string());
    }
    if facts.agent.as_str() == "claude" && matches!(facts.unattended, Fact::Known { value: false }) {
        said.push(words.login_needs_consent().to_string());
    }
    if let Some(drift) = &facts.drift {
        let line = match drift {
            Drift::Changed { location, .. } => words.login_changed(location),
            Drift::Asks { limit } => words.login_asks(limit.current, limit.requested),
        };
        for line in text::wrap_plain(&format!("   {line}"), look.text(Role::Caution), width) {
            rows.push(Row::new(line).item(item));
        }
    }
    if let Some(read) = read {
        said.push(read);
    }
    if !said.is_empty() {
        for line in text::wrap_plain(&format!("   {}", said.join("  ·  ")), look.text(Role::Faint), width) {
            rows.push(Row::new(line).item(item));
        }
    }
    rows
}

/// Each window as `5h 42%`, a window past nine tenths with care, one spent
/// as wrong, one past the reset it was read before as reset; a window of
/// another Codex limit than the first is named by it.
fn limits_line(ctx: &Ctx, windows: &[LimitWindow]) -> Line<'static> {
    let look = &ctx.look;
    let first = windows.first().map(|window| window.name.clone()).unwrap_or_default();
    let mut spans = Vec::new();
    for window in windows {
        if !spans.is_empty() {
            spans.push(Span::styled("  ·  ", look.text(Role::Faint)));
        }
        let span = ctx.words.window(&window.name, window.minutes);
        let claude = ["five_hour", "seven_day", "spend_limit"].iter().any(|name| window.name.starts_with(name));
        let label = if claude || window.name == first || window.minutes.is_none() {
            span
        } else {
            format!("{} {span}", window.name)
        };
        if window.resets_at.is_some_and(|at| at <= ctx.now) {
            spans.push(Span::styled(format!("{label} {}", ctx.words.limit_reset()), look.text(Role::Faint)));
            continue;
        }
        let role = match window.used_tenths {
            1000.. => Role::Wrong,
            900.. => Role::Caution,
            _ => Role::Muted,
        };
        spans.push(Span::styled(format!("{label} {}", percent(window.used_tenths)), look.text(role)));
    }
    Line::from(spans)
}

fn percent(tenths: u32) -> String {
    if tenths > 0 && tenths < 10 { "<1%".to_string() } else { format!("{}%", (tenths + 5) / 10) }
}

fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The line said while this computer's agents are being asked, or why
/// they could not be.
pub fn asking(ctx: &Ctx, failed: Option<&str>) -> Row {
    match failed {
        Some(why) => Row::new(wrong(ctx, ctx.words.not_asked(&text::first_line(why)))),
        None => Row::new(quiet(ctx, ctx.words.asking())),
    }
}
