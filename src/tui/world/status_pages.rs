//! The two single-machine pages: the local service as a chain of steps, and the hub link as
//! figures, a round-trip silhouette and the checks Doctor would print.

use super::model::{self, LinkTone, Stage, Tone, current_status, relative};
use super::widgets::{
    Lines, Tile, clipped, fill, judged, muted, pair, plain, safe, stat_tiles, title_rule, width,
    wrapped,
};
use super::{WorldApp, i18n};
use crate::tui::theme;
use crate::world::control::ReadState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

fn headline(frame: &mut Frame<'_>, area: Rect, tone: Tone, text: &str) -> Rect {
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let mut line = judged(tone.symbol(), tone.color(), text);
    if matches!(tone, Tone::Warn | Tone::Fail) {
        for span in &mut line.spans[1..] {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
    let lines = wrapped(vec![line], usize::from(area.width));
    let rows = (lines.len() as u16)
        .min(area.height.saturating_sub(1))
        .max(1);
    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            height: rows,
            ..area
        },
    );
    Rect {
        y: area.y + rows + 1,
        height: area.height.saturating_sub(rows + 1),
        ..area
    }
}

pub(super) fn service(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
) {
    let Some(snapshot) = &app.snapshot else {
        return;
    };
    let stages = model::stages(snapshot, m);
    // Auto-start is useful, but it is not a prerequisite for a manually started node.
    let chain = stages
        .iter()
        .filter(|stage| stage.stage != Stage::Service)
        .collect::<Vec<_>>();
    let problem = chain
        .iter()
        .find(|stage| matches!(stage.tone, Tone::Fail | Tone::Warn));
    let (tone, title) = problem
        .map(|stage| {
            (
                stage.tone,
                format!("{} · {}", stage.stage.label(m), stage.detail),
            )
        })
        .unwrap_or_else(|| (Tone::Ok, m.service_ok.to_string()));
    let body = headline(frame, area, tone, &title);
    if body.height == 0 {
        return;
    }
    let pill_width = chain
        .iter()
        .map(|stage| width(stage.stage.label(m)).max(width(&stage.detail)) + 4)
        .max()
        .unwrap_or(12) as u16;
    let horizontal = body.height >= 9
        && pill_width * chain.len() as u16 + 4 * (chain.len() as u16 - 1) <= body.width;
    let chain_rows = if horizontal { 3 } else { chain.len() as u16 };
    let parts = Layout::vertical([
        Constraint::Length(chain_rows),
        Constraint::Length(u16::from(body.height > chain_rows + 3)),
        Constraint::Length(if body.height >= chain_rows + 3 { 2 } else { 0 }),
        Constraint::Length(u16::from(body.height > chain_rows + 6)),
        Constraint::Min(0),
    ])
    .split(body);
    if horizontal {
        let mut x = parts[0].x;
        for (index, stage) in chain.iter().enumerate() {
            let rect = Rect::new(x, parts[0].y, pill_width, 2);
            fill(frame.buffer_mut(), rect, theme::surface());
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            format!(" {} ", stage.tone.symbol()),
                            Style::default()
                                .fg(stage.tone.color())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            stage.stage.label(m),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "   {}",
                            clipped(&safe(&stage.detail), usize::from(pill_width) - 3)
                        ),
                        Style::default().fg(theme::muted()),
                    )),
                ]),
                rect,
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    if theme::flat() { "─" } else { "▀" }.repeat(usize::from(pill_width)),
                    Style::default().fg(stage.tone.color()),
                )),
                Rect::new(x, parts[0].y + 2, pill_width, 1),
            );
            if index + 1 < chain.len() {
                frame.render_widget(
                    Paragraph::new(Span::styled(" ──► ", Style::default().fg(theme::border()))),
                    Rect::new(x + pill_width, parts[0].y, 4, 1),
                );
            }
            x += pill_width + 4;
        }
    } else {
        let lines = chain
            .iter()
            .map(|stage| {
                judged(
                    stage.tone.symbol(),
                    stage.tone.color(),
                    format!("{} · {}", stage.stage.label(m), stage.detail),
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines), parts[0]);
    }
    if parts[2].height > 0 {
        let installed = snapshot.service_installed.get().copied();
        let status = current_status(snapshot)
            .filter(|(_, fresh)| *fresh)
            .map(|(status, _)| status);
        let unknown = || m.unknown.to_string();
        let tiles = [
            Tile::new(
                m.stage_service,
                match installed {
                    Some(true) => m.installed,
                    Some(false) => m.not_installed,
                    None => m.unknown,
                },
                match installed {
                    Some(true) => theme::success(),
                    Some(false) => theme::warning(),
                    None => theme::fg(),
                },
            ),
            Tile::new(
                m.ai_sessions,
                status
                    .and_then(|status| status.agent_connections)
                    .map_or_else(unknown, |count| count.to_string()),
                theme::accent(),
            ),
            Tile::new(
                m.running_calls,
                status.map_or_else(unknown, |status| status.running_calls.to_string()),
                theme::accent(),
            ),
            Tile::new(
                m.started,
                status.map_or_else(unknown, |status| relative(&status.started_at, m)),
                theme::fg(),
            ),
            Tile::new(
                m.version,
                status.map_or_else(unknown, |status| status.version.clone()),
                theme::fg(),
            ),
        ];
        stat_tiles(frame, parts[2], &tiles);
    }
    let mut lines: Lines = Vec::new();
    if parts[4].height >= 6 && problem.is_none() {
        lines.push(muted(m.stage_help));
        lines.push(plain(""));
    }
    match &snapshot.service_installed {
        ReadState::Failed(error) => lines.push(judged(
            Tone::Warn.symbol(),
            Tone::Warn.color(),
            format!("{} · {}", m.stage_service, error.detail),
        )),
        ReadState::Missing => lines.push(pair(m.stage_service, m.unknown)),
        ReadState::Ready(_) => {}
    }
    match &snapshot.node {
        ReadState::Failed(error) => {
            lines.push(judged(
                Tone::Fail.symbol(),
                Tone::Fail.color(),
                &error.detail,
            ));
        }
        ReadState::Missing => {
            lines.push(judged(
                Tone::Fail.symbol(),
                Tone::Fail.color(),
                m.unavailable,
            ));
            lines.push(muted(m.unavailable_body));
        }
        ReadState::Ready(_) => {}
    }
    if let Some((status, fresh)) = current_status(snapshot) {
        if !fresh {
            lines.push(judged(Tone::Warn.symbol(), Tone::Warn.color(), m.stale));
            lines.push(pair(m.data_time, relative(&status.written_at, m)));
        }
        lines.push(pair(m.pid, status.pid));
    }
    lines.push(pair(m.log_file, crate::paths::display_path(&app.node_log)));
    super::widgets::scroll(frame, app, parts[4], None, lines);
}

pub(super) fn connection(
    frame: &mut Frame<'_>,
    app: &mut WorldApp,
    m: &i18n::WorldMessages,
    area: Rect,
) {
    let Some(snapshot) = &app.snapshot else {
        return;
    };
    let status = current_status(snapshot);
    let node_available = matches!(snapshot.node, ReadState::Ready(_));
    let (tone, title) = match status {
        _ if !node_available => (
            Tone::Fail,
            if matches!(snapshot.node, ReadState::Missing) {
                m.unavailable.to_string()
            } else {
                m.verdict_node_error.to_string()
            },
        ),
        Some((status, false)) => (
            Tone::Warn,
            format!("{} · {}", m.stale, relative(&status.written_at, m)),
        ),
        Some((status, true)) => {
            let link = LinkTone::from_state(&status.link.state);
            (
                match link {
                    LinkTone::Connected => Tone::Ok,
                    LinkTone::Connecting => Tone::Warn,
                    LinkTone::Stopped => Tone::Fail,
                },
                format!("{} · {}", link.label(m), safe(&status.hub.join(", "))),
            )
        }
        None => (Tone::Info, m.unknown.to_string()),
    };
    let body = headline(frame, area, tone, &title);
    let Some((status, fresh)) = status else {
        frame.render_widget(
            Paragraph::new(wrapped(
                vec![muted(m.unavailable_body)],
                usize::from(body.width),
            )),
            body,
        );
        return;
    };
    let live = fresh && node_available;
    let chart_height = if body.height >= 14 && app.rtt_history.len() >= 2 {
        if body.height >= 20 { 7 } else { 5 }
    } else {
        0
    };
    let tiles_height = if body.height >= 6 { 2 } else { 0 };
    let parts = Layout::vertical([
        Constraint::Length(tiles_height),
        Constraint::Length(u16::from(tiles_height > 0 && body.height > 4)),
        Constraint::Length(chart_height + u16::from(chart_height > 0)),
        Constraint::Length(u16::from(chart_height > 0)),
        Constraint::Min(1),
    ])
    .split(body);
    if tiles_height > 0 {
        let link = &status.link;
        let none = || m.none.to_string();
        let tiles = [
            Tile::new(
                m.rtt,
                link.rtt_ms
                    .filter(|_| live)
                    .map_or_else(|| m.unknown.to_string(), |rtt| format!("{rtt} ms")),
                theme::accent(),
            ),
            Tile::new(
                m.heartbeat,
                link.last_heartbeat_at
                    .as_deref()
                    .map_or_else(none, |at| relative(at, m)),
                theme::fg(),
            ),
            Tile::new(
                m.ack,
                link.last_ack_at
                    .as_deref()
                    .map_or_else(none, |at| relative(at, m)),
                theme::fg(),
            ),
            Tile::new(
                m.outbox,
                link.outbox_depth,
                if link.outbox_depth == 0 {
                    theme::fg()
                } else {
                    theme::warning()
                },
            ),
            Tile::new(
                m.network,
                link.path.clone().unwrap_or_else(|| m.unknown.to_string()),
                theme::fg(),
            ),
        ];
        stat_tiles(frame, parts[0], &tiles);
    }
    if chart_height > 0 {
        let samples = usize::from(parts[2].width.saturating_sub(8)).min(app.rtt_history.len());
        title_rule(
            frame,
            parts[2],
            m.rtt_history,
            Some(&format!(
                "{} · {}",
                i18n::fill(m.latest_samples, &[("n", &samples.to_string())]),
                m.sample_order
            )),
        );
        let chart = Rect {
            y: parts[2].y + 1,
            height: chart_height,
            ..parts[2]
        };
        area_chart(frame, chart, app, theme::success());
    }
    let mut lines = Vec::new();
    if live {
        let findings = crate::world::doctor::link_findings(status)
            .iter()
            .map(|finding| model::finding_view(finding, m))
            .filter(|view| matches!(view.tone, Tone::Warn | Tone::Fail))
            .collect::<Vec<_>>();
        if findings.is_empty() {
            lines.push(judged(
                Tone::Ok.symbol(),
                Tone::Ok.color(),
                m.find_health_ok,
            ));
        }
        for view in findings {
            lines.push(judged(view.tone.symbol(), view.tone.color(), view.text));
            if let Some(remedy) = view.remedy {
                lines.push(muted(format!("  {remedy}")));
            }
        }
        if let Some(node) = snapshot.node.get() {
            for (name, reason) in &node.unverified {
                lines.push(judged(
                    Tone::Warn.symbol(),
                    Tone::Warn.color(),
                    format!("{name}: {reason}"),
                ));
            }
        }
    } else {
        lines.push(muted(m.verdict_stale));
    }
    if let Some(error) = &status.inventory_error {
        lines.push(judged(Tone::Warn.symbol(), Tone::Warn.color(), error));
    }
    if parts[4].height >= 6 {
        lines.push(plain(""));
        lines.push(pair(m.tls, status.link.tls.as_str()));
        lines.push(pair(m.grant_revision, status.grant_revision));
    }
    super::widgets::scroll(frame, app, parts[4], None, lines);
}

/// Recent round trips as a filled silhouette with half-row steps; the newest sample is on the
/// right. Axis labels carry the scale so the shape needs no legend.
fn area_chart(frame: &mut Frame<'_>, area: Rect, app: &WorldApp, color: Color) {
    if area.width < 16 || area.height < 2 {
        return;
    }
    let top = app.rtt_history.iter().copied().max().unwrap_or(1).max(1);
    let label = format!("{top} ms");
    let label_width = width(&label) as u16 + 1;
    let columns = usize::from(area.width - label_width);
    let samples = app
        .rtt_history
        .iter()
        .rev()
        .take(columns)
        .rev()
        .copied()
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return;
    }
    let steps = u64::from(area.height) * 2;
    let buf = frame.buffer_mut();
    let style = Style::default().fg(color);
    // Few samples stretch over the width instead of huddling at one edge.
    for column in 0..columns {
        let value = samples[column * samples.len() / columns];
        let height = ((u64::from(value) * steps).div_ceil(u64::from(top))).max(1);
        let full = (height / 2) as u16;
        let half = height % 2 == 1;
        let x = area.x + label_width + column as u16;
        for row in 0..full {
            if let Some(cell) = buf.cell_mut((x, area.bottom() - 1 - row)) {
                cell.set_symbol("█").set_style(style);
            }
        }
        if half
            && full < area.height
            && let Some(cell) = buf.cell_mut((x, area.bottom() - 1 - full))
        {
            cell.set_symbol("▄").set_style(style);
        }
    }
    let axis = Style::default().fg(theme::muted());
    buf.set_stringn(area.x, area.y, &label, usize::from(label_width), axis);
    buf.set_stringn(
        area.x,
        area.bottom() - 1,
        "0 ms",
        usize::from(label_width),
        axis,
    );
}
