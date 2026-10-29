//! An agent as the user sees it (`world-gui.md`, 用词): named by who it is,
//! "Codex · laptop", with how it came and what it is doing, in the same
//! facts `fastctx w` gives and nothing more.

use fastctx_client::town::busy;
use fastctx_types::residents::{Activity, Origin};
use fastctx_types::town::{ResidentNow, RunningJob};
use fastctx_types::{Address, MachineName};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::Ctx;
use crate::design::Role;
use crate::text;

/// "Codex · laptop".
pub fn name(ctx: &Ctx, agent: &str, machine: &MachineName) -> String {
    format!("{} · {machine}", ctx.words.agent(agent))
}

pub fn resident_name(ctx: &Ctx, resident: &ResidentNow) -> String {
    name(ctx, resident.about.host.agent.as_str(), &resident.about.machine)
}

/// A resident known only by its address: its agent is the start of its id.
pub fn address_name(ctx: &Ctx, address: &Address) -> String {
    let agent = address.resident.as_str().split('-').next().unwrap_or_default();
    name(ctx, agent, &address.machine)
}

/// The mark before an agent: busy breathes, idle is open, asleep is faint.
pub fn mark(ctx: &Ctx, resident: &ResidentNow) -> Span<'static> {
    let activity = resident.now.as_ref().map(|now| now.activity);
    if busy(resident) {
        Span::styled("●", ctx.look.faded(Role::Alive, ctx.breath))
    } else if resident.moved.is_some() || !resident.here || activity == Some(Activity::Asleep) {
        Span::styled("◌", ctx.look.text(Role::Faint))
    } else {
        Span::styled("○", ctx.look.text(Role::Muted))
    }
}

/// Its first line: who it is and how it came; on the right, how long it has
/// been here or where it went.
pub fn first_line(ctx: &Ctx, resident: &ResidentNow) -> (Line<'static>, Line<'static>) {
    let words = &ctx.words;
    let mut spans = vec![
        mark(ctx, resident),
        Span::raw("  "),
        Span::styled(resident_name(ctx, resident), ctx.look.text(Role::Text).add_modifier(Modifier::BOLD)),
    ];
    let origin = match &resident.about.origin {
        Origin::User => words.opened_by_you().to_string(),
        Origin::Colleague { asked_by, label } => {
            format!("{}{}", words.asked_by(&address_name(ctx, asked_by)), words.quoted(label))
        }
    };
    spans.push(Span::styled(format!("   {origin}"), ctx.look.text(Role::Muted)));
    let right = match (&resident.moved, resident.now.as_ref().map(|now| now.activity)) {
        (Some(moved), _) => words.moved_to(moved.to.as_ref().map(|town| town.as_str())),
        (None, Some(Activity::Asleep)) => words.asleep().to_string(),
        (None, _) if resident.here => words.here_for(&words.since(resident.about.born_at, ctx.now)),
        (None, _) => words.left().to_string(),
    };
    (Line::from(spans), Line::from(Span::styled(right, ctx.look.text(Role::Faint))))
}

/// Its second line: its state as its host reports it, what it is doing or
/// did last, and anything wrong.
pub fn second_line(ctx: &Ctx, resident: &ResidentNow, job: Option<&RunningJob>) -> Line<'static> {
    let words = &ctx.words;
    let look = &ctx.look;
    let mut spans = vec![Span::raw("   ")];
    let mut parts: Vec<Span<'static>> = Vec::new();
    if let Some(now) = &resident.now {
        let since = words.since(now.since, ctx.now);
        parts.push(match now.activity {
            Activity::Busy if now.waiting_for_user => Span::styled(words.waiting_for_user(), look.text(Role::Alive)),
            Activity::Busy => Span::styled(format!("{} {since}", words.busy()), look.text(Role::Alive)),
            Activity::Idle => Span::styled(format!("{} {since}", words.idle()), look.text(Role::Muted)),
            Activity::Asleep => Span::styled(format!("{} {since}", words.asleep()), look.text(Role::Faint)),
        });
        if now.desk {
            parts.push(Span::styled(words.using_desk().to_string(), look.text(Role::Muted)));
        }
    }
    parts.push(match (job, &resident.last_action) {
        (Some(job), _) => Span::styled(
            format!(
                "{}{}{}",
                words.running_for(job.machine.as_str(), &words.since(job.started_at, ctx.now)),
                words.colon(),
                text::first_line(&job.command)
            ),
            look.text(Role::Muted),
        ),
        (None, Some(action)) => Span::styled(
            format!(
                "{} {} {} → {}",
                words.ago(action.at, ctx.now),
                action.hand,
                text::first_line(&action.what),
                action.result
            ),
            look.text(Role::Muted),
        ),
        (None, None) => Span::styled(words.no_actions().to_string(), look.text(Role::Faint)),
    });
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", look.text(Role::Faint)));
        }
        spans.push(part);
    }
    if let Some(trouble) = resident.now.as_ref().and_then(|now| now.trouble.as_ref()) {
        spans.push(Span::styled(
            format!("    ▲ {}{}", words.troubled(), words.aside(&words.ago(trouble.at, ctx.now))),
            look.text(Role::Caution),
        ));
    }
    Line::from(spans)
}
