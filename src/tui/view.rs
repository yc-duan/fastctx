//! Adaptive ratatui views and the shared TUI theme.

use super::app::{ActiveHost, App, Screen, StatusState};
use super::budget_editor::{self, ToolBudgetInputError};
use super::config::{
    self, ConfigGroupId, ConfigItemId, ConfigItemRole, ConfigListRow, ConfigValue,
};
use super::jobs::{JobGroup, JobsState, display_output_line, grouped_jobs, source_count};
use super::theme;
use crate::control::apply::{PreviewAction, PreviewItem, PreviewTarget};
use crate::control::doctor::DoctorCheckStatus;
use crate::control::i18n::ALL_LANGUAGES;
use crate::control::link::LinkState;
use crate::control::settings::{
    MAX_REPLACE_FILE_LIMIT_MIB, MIN_REPLACE_FILE_LIMIT_MIB, Tier, ToolBudgetLevel,
};
use crate::control::transaction::FileAction;
use crate::search_parallelism::{self, SearchParallelismInputError};
use crate::shell::jobs::{JobSourceSummary, JobSummary};
use crate::update::{NpmDiscovery, NpmVersionAuthority, StartupUpdate};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
    Table, Wrap,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const MIN_TERMINAL_WIDTH: u16 = 40;
const MIN_TERMINAL_HEIGHT: u16 = 9;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    app.detail_viewport.enter(app.screen);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().fg(theme::fg()).bg(theme::bg())),
        area,
    );
    if area.width < MIN_TERMINAL_WIDTH || area.height < MIN_TERMINAL_HEIGHT {
        render_minimum_size(frame, app, area);
        return;
    }
    let footer_height = 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(footer_height),
        ])
        .split(area);
    render_header(frame, app, vertical[0]);
    if uses_narrow_layout(area, app.screen) {
        render_narrow(frame, app, vertical[1]);
    } else {
        render_body(frame, app, vertical[1]);
    }
    render_footer(frame, app, vertical[2]);
    if let Some(toast) = &app.toast {
        render_toast(
            frame,
            vertical[1],
            &toast.message,
            if toast.warning {
                theme::warning()
            } else {
                theme::success()
            },
        );
    }
}

fn uses_narrow_layout(area: Rect, screen: Screen) -> bool {
    match screen {
        Screen::MigrationNotice
        | Screen::Config
        | Screen::ConfigCpuEdit
        | Screen::ConfigOutputGuardConfirm
        | Screen::ConfigDiscardConfirm
        | Screen::ConfigResetConfirm
        | Screen::Jobs
        | Screen::Update
        | Screen::UpdateConfirm => false,
        Screen::ApplyPreview
        | Screen::UnapplyPreview
        | Screen::Status
        | Screen::Receipt
        | Screen::About
        | Screen::OperationFailed
        | Screen::JobsKillFailed => area.width < 72 || area.height < 24,
        _ => area.width < 52 || area.height < 12,
    }
}

fn render_minimum_size(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let message = app
        .messages()
        .narrow_terminal
        .replace("{width}", &MIN_TERMINAL_WIDTH.to_string())
        .replace("{height}", &MIN_TERMINAL_HEIGHT.to_string());
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme::fg()))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " FastCtx",
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(theme::border())),
        Span::styled(
            app.messages().app_title,
            Style::default().fg(theme::muted()),
        ),
    ]);
    let version = Line::from(Span::styled(
        format!("v{}  ", env!("CARGO_PKG_VERSION")),
        Style::default().fg(theme::muted()),
    ))
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(title).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme::border())),
        ),
        area,
    );
    frame.render_widget(version, area);
}

fn render_body(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    match app.screen {
        Screen::MigrationNotice => render_migration_notice(frame, app, area),
        Screen::Update => render_update(frame, app, area),
        Screen::UpdateChecking => render_loading(frame, app, area, app.update_messages().checking),
        Screen::UpdateConfirm => render_update_confirmation(frame, app, area),
        Screen::Language { .. } => render_languages(frame, app, area),
        Screen::Main => render_main(frame, app, area),
        Screen::Connections => render_connections(frame, app, area),
        Screen::ApplyHome => render_apply_home(frame, app, area),
        Screen::ApplyLoading | Screen::UnapplyLoading => {
            render_loading(frame, app, area, app.messages().loading)
        }
        Screen::ApplyPreview => {
            if app.active_host == ActiveHost::DeepSeekHarness {
                render_dsh_preview(frame, app, area, true)
            } else {
                render_preview(frame, app, area, true)
            }
        }
        Screen::ApplyConflict => render_confirmation(
            frame,
            app,
            area,
            app.messages().conflict_warning,
            theme::warning(),
        ),
        Screen::ApplyConfirm => render_confirmation(
            frame,
            app,
            area,
            app.messages().confirm_apply,
            theme::accent(),
        ),
        Screen::ApplyRunning | Screen::UnapplyRunning => {
            render_loading(frame, app, area, app.messages().loading)
        }
        Screen::UnapplyPreview => {
            if app.active_host == ActiveHost::DeepSeekHarness {
                render_dsh_preview(frame, app, area, false)
            } else {
                render_preview(frame, app, area, false)
            }
        }
        Screen::UnapplyConfirm => render_confirmation(
            frame,
            app,
            area,
            if app.active_host == ActiveHost::All {
                "Disconnect every host and delete all shared FastCtx data?"
            } else {
                app.messages().confirm_unapply
            },
            theme::danger(),
        ),
        Screen::Config => render_config(frame, app, area),
        Screen::ConfigCpuEdit => render_cpu_limit_editor(frame, app, area),
        Screen::ConfigBudgetEdit(item) => render_budget_editor(frame, app, area, item),
        Screen::ConfigOutputGuardConfirm => render_confirmation(
            frame,
            app,
            area,
            app.guard_messages().disable_confirm,
            theme::danger(),
        ),
        Screen::ConfigDiscardConfirm => render_confirmation(
            frame,
            app,
            area,
            app.config_messages().discard_confirm,
            theme::danger(),
        ),
        Screen::ConfigResetConfirm => {
            let prompt = format!(
                "{}\n{}",
                app.config_messages().reset_confirm,
                app.config_messages().reset_all_note
            );
            render_confirmation(frame, app, area, &prompt, theme::danger());
        }
        Screen::ConfigResetting => {
            render_loading(frame, app, area, app.config_messages().reset_all_label)
        }
        Screen::ConfigSaving => {
            render_loading(frame, app, area, app.config_messages().saving_notice)
        }
        Screen::Jobs => render_jobs(frame, app, area),
        Screen::JobsKillConfirm => render_job_kill_confirmation(frame, app, area),
        Screen::JobsKilling => render_loading(frame, app, area, app.job_messages().loading),
        Screen::JobsKillFailed => render_error(frame, app, area),
        Screen::Status => render_status(frame, app, area),
        Screen::About => render_about(frame, app, area),
        Screen::Receipt => render_receipt(frame, app, area),
        Screen::OperationFailed => render_error(frame, app, area),
    }
}

fn render_update(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let messages = app.update_messages();
    let mut lines = Vec::new();
    let primary = if let StartupUpdate::Available(plan) = &app.update_state {
        lines.push(Line::styled(
            messages.available_title,
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ));
        append_current_version(&mut lines);
        append_styled_text_lines(
            &mut lines,
            &messages
                .available_body
                .replace("{current}", env!("CARGO_PKG_VERSION"))
                .replace("{latest}", plan.target_version())
                .replace("{source}", &plan.source_label()),
            Style::default().fg(theme::fg()),
        );
        if let Some(discovery) = plan.npm_discovery() {
            append_npm_discovery_lines(&mut lines, discovery, app, area.width);
        }
        messages.action_update
    } else {
        match &app.update_state {
            StartupUpdate::NpmCurrent { discovery } => {
                lines.push(Line::styled(
                    messages.up_to_date,
                    Style::default().fg(theme::success()),
                ));
                append_current_version(&mut lines);
                append_npm_discovery_lines(&mut lines, discovery, app, area.width);
            }
            StartupUpdate::NpmPending {
                target_version,
                discovery,
            } => {
                lines.push(Line::styled(
                    messages.pending_title,
                    Style::default()
                        .fg(theme::warning())
                        .add_modifier(Modifier::BOLD),
                ));
                append_current_version(&mut lines);
                append_styled_text_lines(
                    &mut lines,
                    &messages
                        .pending_body
                        .replace("{latest}", target_version)
                        .replace(
                            "{registry}",
                            discovery
                                .probes
                                .iter()
                                .filter_map(|probe| probe.latest_version.as_deref())
                                .max()
                                .unwrap_or("unknown"),
                        ),
                    Style::default().fg(theme::warning()),
                );
                append_npm_discovery_lines(&mut lines, discovery, app, area.width);
            }
            StartupUpdate::Failed(error) => {
                lines.push(Line::styled(
                    format!("{}: {}", messages.check_failed, error.message),
                    Style::default().fg(theme::danger()),
                ));
                append_current_version(&mut lines);
            }
            StartupUpdate::InstallFailed(error) => {
                lines.push(Line::styled(
                    format!("{}: {error}", messages.update_failed),
                    Style::default().fg(theme::danger()),
                ));
                append_current_version(&mut lines);
            }
            StartupUpdate::None => {
                lines.push(Line::styled(
                    messages.up_to_date,
                    Style::default().fg(theme::muted()),
                ));
                append_current_version(&mut lines);
            }
            StartupUpdate::Available(_) => unreachable!("available was handled above"),
        }
        messages.action_check
    };
    let action_lines = labeled_action_lines(
        app.selected,
        primary,
        messages.action_continue,
        area.width.saturating_sub(4),
    );
    let action_height = u16::try_from(action_lines.len()).unwrap_or(u16::MAX);
    let vertical_padding = u16::from(area.height >= action_height.saturating_add(5));
    let popup = inner(area, 2, vertical_padding);
    let body_has_panel = popup.height >= action_height.saturating_add(3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(if body_has_panel { 3 } else { 1 }),
            Constraint::Length(action_height),
        ])
        .split(popup);
    let visible_rows = usize::from(if body_has_panel {
        chunks[0].height.saturating_sub(2)
    } else {
        chunks[0].height
    });
    app.detail_viewport.update(lines.len(), visible_rows);
    let mut body = Paragraph::new(lines)
        .style(Style::default().fg(theme::fg()))
        .scroll((app.detail_viewport.offset() as u16, 0));
    if body_has_panel {
        body = body
            .block(panel(messages.page_title).border_style(Style::default().fg(theme::accent())));
    }
    frame.render_widget(body, chunks[0]);
    frame.render_widget(
        Paragraph::new(action_lines).alignment(Alignment::Center),
        chunks[1],
    );
}

fn append_current_version(lines: &mut Vec<Line<'static>>) {
    lines.push(detail_line(
        "FastCtx",
        &format!("v{}", env!("CARGO_PKG_VERSION")),
    ));
    lines.push(Line::raw(""));
}

fn append_styled_text_lines(lines: &mut Vec<Line<'static>>, text: &str, style: Style) {
    lines.extend(
        text.split('\n')
            .map(|line| Line::styled(line.to_string(), style)),
    );
}

fn append_npm_discovery_lines(
    lines: &mut Vec<Line<'static>>,
    discovery: &NpmDiscovery,
    app: &App,
    width: u16,
) {
    lines.push(Line::raw(""));
    lines.push(detail_line("update.source", &discovery.source_policy));
    lines.push(detail_line(
        "Version authority",
        match discovery.authority {
            NpmVersionAuthority::Official => "GitHub / official npm",
            NpmVersionAuthority::MirrorFallback => "mirror fallback (official unavailable)",
        },
    ));
    lines.push(Line::styled(
        app.update_messages().sources_title,
        Style::default()
            .fg(theme::fg())
            .add_modifier(Modifier::BOLD),
    ));
    let limit = usize::from(width.saturating_sub(12)).max(16);
    for probe in &discovery.probes {
        let marker = if probe.is_ready() {
            "✓"
        } else if probe.reachable {
            "◐"
        } else {
            "×"
        };
        let color = if probe.is_ready() {
            theme::success()
        } else if probe.reachable {
            theme::warning()
        } else {
            theme::danger()
        };
        let selected = discovery.selected_registry.as_deref() == Some(probe.registry.as_str());
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(color)),
            Span::styled(
                probe.source_name.clone(),
                Style::default()
                    .fg(if selected {
                        theme::accent()
                    } else {
                        theme::fg()
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                format!("  {}", truncate_end(&probe.registry, limit)),
                Style::default().fg(theme::muted()),
            ),
        ]));
        let latest = probe.latest_version.as_deref().unwrap_or("—");
        lines.push(Line::styled(
            format!(
                "   latest v{latest} · fastctx {} · {} {}",
                check_mark(probe.main_package_ready),
                discovery.platform_package,
                check_mark(probe.platform_package_ready)
            ),
            Style::default().fg(theme::muted()),
        ));
        if let Some(error) = &probe.error {
            lines.push(Line::styled(
                format!("   {}", truncate_end(error, limit)),
                Style::default().fg(color),
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        discovery.selection_reason.clone(),
        Style::default().fg(theme::muted()),
    ));
}

const fn check_mark(value: bool) -> &'static str {
    if value { "✓" } else { "×" }
}

fn render_update_confirmation(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(plan) = (match &app.update_state {
        StartupUpdate::Available(plan) => Some(plan),
        _ => None,
    }) else {
        render_message_panel(
            frame,
            area,
            app.update_messages().check_failed,
            app.messages().operation_failed,
            theme::danger(),
        );
        return;
    };
    let prompt = app
        .update_messages()
        .available_body
        .replace("{current}", env!("CARGO_PKG_VERSION"))
        .replace("{latest}", plan.target_version())
        .replace("{source}", &plan.source_label());
    render_confirmation(frame, app, area, &prompt, theme::accent());
}

fn labeled_action_lines(
    selected: usize,
    primary: &str,
    secondary: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let style = |active: bool, color| {
        if active {
            Style::default()
                .fg(theme::bg())
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        }
    };
    let standard_primary = Span::styled(
        format!("  {primary}  "),
        style(selected == 0, theme::accent()),
    );
    let standard_secondary = Span::styled(
        format!("  {secondary}  "),
        style(selected == 1, theme::muted()),
    );
    let inline = Line::from(vec![
        standard_primary.clone(),
        Span::raw("     "),
        standard_secondary.clone(),
    ]);
    if inline.width() <= usize::from(width) {
        return vec![inline];
    }

    let compact_primary = Span::styled(
        format!(" {primary} "),
        style(selected == 0, theme::accent()),
    );
    let compact_secondary = Span::styled(
        format!(" {secondary} "),
        style(selected == 1, theme::muted()),
    );
    let compact = Line::from(vec![
        compact_primary.clone(),
        Span::raw("   "),
        compact_secondary.clone(),
    ]);
    if compact.width() <= usize::from(width) {
        vec![compact]
    } else {
        vec![Line::from(compact_primary), Line::from(compact_secondary)]
    }
}

fn render_languages(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(inner(area, 2, 1));
    let items = ALL_LANGUAGES
        .iter()
        .map(|language| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<6}", language.code()),
                    Style::default().fg(theme::muted()),
                ),
                Span::raw(language.native_name()),
            ]))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(app.messages().language_title))
            .highlight_style(selected_style())
            .highlight_symbol("❯ "),
        chunks[0],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                app.messages().language_prompt,
                Style::default()
                    .fg(theme::fg())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                ALL_LANGUAGES[app.selected].native_name(),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                ALL_LANGUAGES[app.selected].code(),
                Style::default().fg(theme::muted()),
            ),
        ])
        .block(panel("FastCtx"))
        .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

fn render_main(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(inner(area, 2, 1));
    let messages = app.messages();
    let labels = [
        messages.menu_apply,
        messages.menu_config,
        app.job_messages().menu,
        app.update_messages().page_title,
        messages.menu_status,
        messages.menu_about,
        messages.menu_language,
    ];
    let items = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let requires_action = index == 0 && app.link_state.requires_apply();
            let item = ListItem::new(format!(" {}", main_menu_label(app, index, label)));
            if requires_action {
                item.style(
                    Style::default()
                        .fg(theme::warning())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(messages.main_title))
            .highlight_style(selected_style())
            .highlight_symbol("❯"),
        chunks[0],
        &mut state,
    );
    // The panel reports the connection rather than echoing the highlighted entry: the cursor
    // already names that, and the one thing the menu cannot otherwise show is whether FastCtx is
    // still connected to the host with the guidance this build writes.
    let mut details = vec![link_status_line(app)];
    if app.link_state.requires_apply() {
        details.push(Line::styled(
            messages.link_state_stale_hint,
            Style::default().fg(theme::muted()),
        ));
    }
    details.extend([
        Line::raw(""),
        detail_line("FastCtx", &format!("v{}", env!("CARGO_PKG_VERSION"))),
        detail_line(messages.tier_label, app.settings.tier.display_name()),
        detail_line(messages.menu_language, app.language.native_name()),
    ]);
    if app.selected == 2 {
        let count = app
            .running_job_count
            .map(|count| {
                app.job_messages()
                    .running_count
                    .replace("{count}", &count.to_string())
            })
            .unwrap_or_else(|| "—".to_string());
        details.push(detail_line(app.job_messages().title, &count));
    }
    if app.selected == 3 {
        details.push(detail_line(
            app.update_messages().page_title,
            &update_state_summary(app),
        ));
    }
    // Users have read the control terminal as the service itself and kept it open; this is the
    // only place that says otherwise.
    details.push(Line::raw(""));
    details.push(Line::styled(
        messages.terminal_role_note,
        Style::default().fg(theme::muted()),
    ));
    frame.render_widget(
        Paragraph::new(details)
            .block(panel(messages.app_title))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

fn render_connections(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(inner(area, 2, 1));
    let codex = if app.settings.integrations.codex.is_some() {
        "Connected"
    } else {
        "Not connected"
    };
    let dsh = app
        .dsh_status
        .as_ref()
        .map(|(state, _)| state.clone())
        .unwrap_or_else(|_| "unhealthy".to_string());
    let items = [
        format!(" ChatGPT / Codex          {codex}"),
        format!(" DeepSeek Harness         {dsh}"),
        " Disconnect all".to_string(),
    ]
    .into_iter()
    .map(ListItem::new)
    .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel("Connections"))
            .highlight_style(selected_style())
            .highlight_symbol("❯"),
        chunks[0],
        &mut state,
    );
    let details = if app.selected == 0 {
        vec![
            detail_line("Host", "ChatGPT / Codex"),
            detail_line("Home", &crate::paths::display_path(&app.paths.codex_dir)),
            detail_line("Scope", "Selected Codex profile"),
        ]
    } else if app.selected == 1 {
        vec![
            detail_line("Host", "DeepSeek Harness"),
            detail_line("Home", &crate::paths::display_path(&app.paths.dsh_dir)),
            detail_line("Source", app.paths.dsh_home_source.as_str()),
            detail_line("Patch", &crate::paths::display_path(&app.paths.dsh_patch)),
            detail_line("Timeout", "300000 ms"),
            detail_line("Scope", "Host-wide (all DSH profiles)"),
        ]
    } else {
        vec![
            detail_line("Action", "Disconnect every host"),
            detail_line("Shared data", "Delete FastCtx settings, binary, and jobs"),
            Line::styled(
                "A separate destructive confirmation is required.",
                Style::default().fg(theme::danger()),
            ),
        ]
    };
    frame.render_widget(
        Paragraph::new(details)
            .block(panel("Connection details"))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

/// Marks the action that repairs a stale connection in text, so it stays visible without colour.
fn main_menu_label(app: &App, index: usize, label: &str) -> String {
    if index == 0 && app.link_state.requires_apply() {
        format!("! {label}")
    } else {
        label.to_string()
    }
}

/// One line naming the connection to the host, shared by the main menu and the connect page so
/// the two can never disagree about what "connected" means. The symbol carries the state on its
/// own because monochrome terminals have no colour to read (R-22).
fn link_status_line(app: &App) -> Line<'static> {
    let messages = app.messages();
    let (symbol, text, colour) = match app.link_state {
        LinkState::Absent => ("○", messages.link_state_absent, theme::accent()),
        LinkState::Current => ("✓", messages.link_state_current, theme::success()),
        LinkState::ApplyRequired
        | LinkState::KnownLegacy
        | LinkState::Missing
        | LinkState::Drifted
        | LinkState::Malformed
        | LinkState::Unreadable => ("!", messages.link_state_stale, theme::warning()),
    };
    Line::styled(
        format!("{symbol} {text}"),
        Style::default().fg(colour).add_modifier(Modifier::BOLD),
    )
}

fn detail_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}  "), Style::default().fg(theme::muted())),
        Span::styled(value.to_string(), Style::default().fg(theme::fg())),
    ])
}

fn update_state_summary(app: &App) -> String {
    match &app.update_state {
        StartupUpdate::Available(plan) => {
            format!("v{} · {}", plan.target_version(), plan.source_label())
        }
        StartupUpdate::NpmCurrent { discovery } => discovery
            .selected_source
            .as_deref()
            .map(|source| format!("v{} · {source}", discovery.target_version))
            .unwrap_or_else(|| format!("v{} · current", discovery.target_version)),
        StartupUpdate::NpmPending { target_version, .. } => {
            format!("v{target_version} · propagation pending")
        }
        StartupUpdate::Failed(error) => format!("check failed · {}", error.message),
        StartupUpdate::InstallFailed(error) => format!("update failed · {error}"),
        StartupUpdate::None => format!("v{} · current", env!("CARGO_PKG_VERSION")),
    }
}

fn tier_note(app: &App, tier: Tier) -> &'static str {
    match tier {
        Tier::Compact => app.messages().tier_note_compact,
        Tier::Standard => app.messages().tier_note_standard,
        Tier::High => app.messages().tier_note_high,
    }
}

/// Compact stays muted, Standard uses neutral emphasis, and High uses amber caution.
/// Green is deliberately avoided so a larger output limit never reads as inherently better.
fn tier_color(tier: Tier) -> Color {
    match tier {
        Tier::Compact => theme::muted(),
        Tier::Standard => theme::accent(),
        Tier::High => theme::warning(),
    }
}

fn render_apply_home(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let messages = app.messages();
    let items = vec![
        ListItem::new(format!(" {}", messages.action_apply)),
        ListItem::new(format!(" {}", messages.action_unapply)),
        ListItem::new(" Doctor"),
    ];
    let mut state = ListState::default().with_selected(Some(app.selected));
    let saved_budgets = app.settings.tool_budgets.resolve(app.settings.tier);
    let saved_global = app.settings.tier.fastctx_budget();
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(inner(area, 2, 1));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(messages.apply_title))
            .highlight_style(selected_style())
            .highlight_symbol("❯"),
        chunks[0],
        &mut state,
    );
    let mut details = if app.active_host == ActiveHost::DeepSeekHarness {
        let state = app
            .dsh_status
            .as_ref()
            .map(|(state, _)| state.clone())
            .unwrap_or_else(|_| "unhealthy".to_string());
        vec![detail_line("DeepSeek Harness", &state)]
    } else {
        vec![link_status_line(app)]
    };
    if app.link_state.requires_apply() {
        details.push(Line::styled(
            messages.link_state_stale_hint,
            Style::default().fg(theme::muted()),
        ));
    }
    details.extend([
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                format!("{}  ", messages.tier_label),
                Style::default().fg(theme::muted()),
            ),
            Span::styled(
                app.settings.tier.display_name(),
                Style::default()
                    .fg(tier_color(app.settings.tier))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {} / {}",
                    app.settings.tier.host_limit(),
                    app.settings.tier.fastctx_budget()
                ),
                Style::default().fg(theme::muted()),
            ),
        ]),
        detail_line(
            "inspect_local_file",
            &budget_summary(saved_budgets.read, saved_global),
        ),
        detail_line("grep", &budget_summary(saved_budgets.grep, saved_global)),
        detail_line("glob", &budget_summary(saved_budgets.glob, saved_global)),
        detail_line("run", &budget_summary(saved_budgets.run, saved_global)),
        detail_line(
            "job_output",
            &budget_summary(saved_budgets.job_output, saved_global),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(details).block(panel(messages.config_title)),
        chunks[1],
    );
}

fn render_dsh_preview(frame: &mut Frame<'_>, app: &mut App, area: Rect, apply: bool) {
    let Some(plan) = app.dsh_plan.as_ref() else {
        render_loading(frame, app, area, app.messages().empty);
        return;
    };
    let mut lines = vec![
        detail_line("Host", "DeepSeek Harness"),
        detail_line("Scope", "Host-wide (all DSH profiles)"),
        detail_line("Home", &crate::paths::display_path(&plan.dsh_home)),
        detail_line("Timeout", "300000 ms"),
        Line::raw(""),
    ];
    for change in plan.preview_changes() {
        let (verb, colour) = if !change.is_changed() {
            ("Unchanged", theme::muted())
        } else {
            match change.action {
                FileAction::Write(_) if change.original.is_none() => ("Create", theme::accent()),
                FileAction::Write(_) => ("Update", theme::accent()),
                FileAction::Delete => ("Delete", theme::danger()),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{verb:<10}"), Style::default().fg(colour)),
            Span::styled(
                crate::paths::display_path(&change.target),
                Style::default().fg(theme::fg()),
            ),
        ]));
    }
    if !apply && plan.running_jobs() > 0 {
        lines.push(Line::styled(
            format!(
                "Stop      {} running background job(s)",
                plan.running_jobs()
            ),
            Style::default().fg(theme::danger()),
        ));
    }
    if !apply && plan.running_processes() > 0 {
        lines.push(Line::styled(
            format!(
                "Stop      {} running FastCtx process(es)",
                plan.running_processes()
            ),
            Style::default().fg(theme::danger()),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        if apply {
            "New DeepSeek Harness sessions will use this connection."
        } else if app.settings.integrations.codex.is_some() {
            "ChatGPT / Codex and the shared FastCtx installation will be preserved."
        } else {
            "This is the last connection; shared FastCtx data will also be removed."
        },
        Style::default().fg(theme::muted()),
    ));
    let preview_area = inner(area, 2, 1);
    app.detail_viewport.update(
        lines.len(),
        usize::from(preview_area.height.saturating_sub(2)),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app.messages().preview_title))
            .scroll((
                u16::try_from(app.detail_viewport.offset()).unwrap_or(u16::MAX),
                0,
            ))
            .wrap(Wrap { trim: false }),
        preview_area,
    );
}

fn preview_purpose(app: &App, target: PreviewTarget) -> &'static str {
    match target {
        PreviewTarget::Binary => app.messages().purpose_binary,
        PreviewTarget::CodexConfig => app.messages().purpose_codex_config,
        PreviewTarget::Agents => app.messages().purpose_agents,
        PreviewTarget::Receipt => app.messages().purpose_receipt,
    }
}

fn preview_verb(app: &App, action: PreviewAction) -> (&'static str, Color) {
    let messages = app.messages();
    match action {
        PreviewAction::Install => (messages.verb_install, theme::accent()),
        PreviewAction::Modify => (messages.verb_modify, theme::accent()),
        PreviewAction::Record => (messages.verb_record, theme::accent()),
        PreviewAction::Delete => (messages.verb_delete, theme::danger()),
        PreviewAction::Keep => (messages.verb_keep, theme::warning()),
        PreviewAction::Unchanged => (messages.label_unchanged, theme::muted()),
    }
}

fn push_preview_card(lines: &mut Vec<Line<'static>>, app: &App, item: &PreviewItem) {
    let (verb, color) = preview_verb(app, item.action);
    let unchanged = item.action == PreviewAction::Unchanged;
    lines.push(Line::from(vec![
        Span::styled(
            if unchanged { "○ " } else { "● " },
            Style::default().fg(color),
        ),
        Span::styled(
            verb,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", crate::paths::display_path(&item.path)),
            Style::default().fg(if unchanged {
                theme::muted()
            } else {
                theme::fg()
            }),
        ),
    ]));
    // A muted purpose line under each target explains what that change accomplishes.
    lines.push(Line::styled(
        format!("    {}", preview_purpose(app, item.target)),
        Style::default().fg(theme::muted()),
    ));
    if item.action == PreviewAction::Keep {
        lines.push(Line::styled(
            format!("    {}", app.messages().manual_cleanup_note),
            Style::default().fg(theme::muted()),
        ));
    }
    for detail in &item.details {
        // The shared host limit affects every tool, so its changes need the warning color.
        let shared_limit_change =
            detail.text.starts_with("tool_output_token_limit") && detail.text.contains('→');
        let style = if detail.removed {
            // Strikethrough plus the danger color keeps disappearing entries unambiguous.
            Style::default()
                .fg(theme::danger())
                .add_modifier(Modifier::CROSSED_OUT)
        } else if shared_limit_change {
            Style::default().fg(theme::warning())
        } else {
            Style::default().fg(theme::muted())
        };
        lines.push(Line::styled(format!("    {}", detail.text), style));
        if shared_limit_change {
            lines.push(Line::styled(
                format!("      {}", app.messages().conflict_warning),
                Style::default().fg(theme::warning()),
            ));
        }
    }
}

fn render_preview(frame: &mut Frame<'_>, app: &mut App, area: Rect, apply: bool) {
    let running_processes = if apply {
        None
    } else {
        app.unapply_plan
            .as_ref()
            .map(|plan| plan.running_processes())
    };
    let items = if apply {
        app.apply_plan.as_ref().map(|plan| plan.preview())
    } else {
        app.unapply_plan.as_ref().map(|plan| plan.preview())
    };
    let Some(items) = items else {
        render_loading(frame, app, area, app.messages().empty);
        return;
    };
    let has_changes = items
        .iter()
        .any(|item| !matches!(item.action, PreviewAction::Unchanged))
        || running_processes.is_some_and(|count| count > 0);
    let mut lines = Vec::new();
    if !has_changes {
        lines.push(Line::styled(
            app.messages().no_changes,
            Style::default()
                .fg(theme::success())
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
    }
    for item in items {
        push_preview_card(&mut lines, app, item);
        lines.push(Line::raw(""));
    }
    if let Some(count) = running_processes {
        let changed = count > 0;
        let color = if changed {
            theme::danger()
        } else {
            theme::muted()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if changed { "● " } else { "○ " },
                Style::default().fg(color),
            ),
            Span::styled(
                app.unapply_processes_message()
                    .replace("{count}", &count.to_string()),
                Style::default().fg(color).add_modifier(if changed {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
        lines.push(Line::raw(""));
    }
    if has_changes {
        lines.push(Line::styled(
            app.messages().restart_notice,
            Style::default().fg(theme::muted()),
        ));
    }
    let preview_area = inner(area, 2, 1);
    app.detail_viewport.update(
        lines.len(),
        usize::from(preview_area.height.saturating_sub(2)),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app.messages().preview_title))
            .scroll((
                u16::try_from(app.detail_viewport.offset()).unwrap_or(u16::MAX),
                0,
            ))
            .wrap(Wrap { trim: false }),
        preview_area,
    );
}

fn render_config(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let messages = app.messages();
    let compact = area.height < 8;
    let content_area = if compact {
        inner(area, 1, 0)
    } else {
        inner(area, 2, 1)
    };
    const DETAIL_ROWS_MAX: u16 = 9;
    let detail_height = if compact {
        2
    } else {
        content_area
            .height
            .saturating_sub(6)
            .clamp(4, DETAIL_ROWS_MAX)
    };
    // The save button holds a row of its own between the list and the detail pane, but only once
    // the detail pane has all it can use. Below that every row belongs to the settings themselves
    // and the footer still names the save key, so the action stays reachable unbuttoned.
    //
    // Riding the panel's bottom border would have cost no row at all, except that a centred border
    // title truncates wide characters, and a button nobody can read in Japanese is not a button.
    let button_height = u16::from(!compact && detail_height == DETAIL_ROWS_MAX);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(button_height),
            Constraint::Length(detail_height),
        ])
        .split(content_area);
    let visible_rows = usize::from(if compact {
        chunks[0].height
    } else {
        chunks[0].height.saturating_sub(2)
    });
    let list_rows = config::list_rows();
    let window = app
        .config_viewport
        .window(app.config_cursor, list_rows.len(), visible_rows);
    let mut table_rows = Vec::new();
    if window.show_above {
        table_rows.push(config_more_row(messages.config_more_above));
    }
    for row in &list_rows[window.start..window.end] {
        match *row {
            // Only pinned groups render without a heading and those never enter this list, so the
            // empty fallback is unreachable; it stays a blank row rather than a skipped one so a
            // future headless group could never desynchronise the viewport's row arithmetic.
            ConfigListRow::Group(group) => table_rows.push(Row::new(vec![
                Cell::from(Line::styled(
                    config::group_title(
                        group,
                        messages,
                        app.config_messages(),
                        app.guard_messages(),
                        app.update_messages(),
                    )
                    .unwrap_or_default()
                    .to_string(),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(""),
            ])),
            ConfigListRow::Item(entry) => {
                table_rows.push(config_item_row(app, entry.group, entry.item, entry.role));
            }
        }
    }
    if window.show_below {
        table_rows.push(config_more_row(messages.config_more_below));
    }
    // The value column carries the scroll hint as well as the values, and its longest
    // translations need every column of this share; widening the item column to fit the
    // longest tool identifier on a narrow terminal clips those hints instead. Both columns
    // clip below roughly 70 columns, which is the terminal's limit rather than a layout choice.
    let mut table = Table::new(
        table_rows,
        [Constraint::Percentage(42), Constraint::Percentage(58)],
    );
    table = table.column_spacing(if compact { 1 } else { 2 });
    let title = if app.config_is_dirty() {
        format!(
            "{} · {}",
            messages.menu_config,
            app.config_messages().unsaved_changes
        )
    } else {
        messages.menu_config.to_string()
    };
    if !compact {
        table = table.block(panel(&title));
    }
    frame.render_widget(table, chunks[0]);
    if chunks[1].height > 0 {
        frame.render_widget(
            Paragraph::new(config_save_button_line(app)).alignment(Alignment::Center),
            chunks[1],
        );
    }

    let entry = app.config_cursor.entry();
    let guarded = app.output_guard_active();
    let effective_output = app.effective_output();
    let detail = match app.config_draft.value_with_guard(entry.item, guarded) {
        ConfigValue::Tier(tier) => vec![
            Line::from(vec![
                Span::styled(
                    tier.display_name(),
                    Style::default()
                        .fg(tier_color(tier))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", tier_note(app, tier)),
                    Style::default().fg(theme::fg()),
                ),
            ]),
            Line::styled(
                messages.tier_definition,
                Style::default().fg(theme::muted()),
            ),
            Line::raw(""),
            Line::styled(
                format!(
                    "tool_output_token_limit {} · FASTCTX_TOKEN_BUDGET {} · Codex default 10000",
                    tier.host_limit(),
                    tier.fastctx_budget()
                ),
                Style::default().fg(theme::muted()),
            ),
            Line::styled(
                messages.tier_values_note,
                Style::default().fg(theme::muted()),
            ),
            Line::raw(""),
            Line::styled(messages.tier_explainer, Style::default().fg(theme::muted())),
        ],
        ConfigValue::GuardedTier(selected_tier) => vec![
            Line::from(vec![
                Span::styled(
                    "Guarded",
                    Style::default()
                        .fg(theme::warning())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ·  {}", selected_tier.display_name()),
                    Style::default().fg(theme::muted()),
                ),
            ]),
            Line::styled(
                app.guard_messages().locked_note,
                Style::default().fg(theme::fg()),
            ),
            Line::raw(""),
            Line::styled(
                format!(
                    "tool_output_token_limit {} · FASTCTX_TOKEN_BUDGET {}",
                    effective_output.host_limit, effective_output.fastctx_budget
                ),
                Style::default().fg(theme::muted()),
            ),
        ],
        ConfigValue::Budget(budget) => {
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    config::item_label(
                        config::group_spec(entry.group).parent(),
                        messages,
                        app.config_messages(),
                        app.guard_messages(),
                        app.job_messages(),
                        app.update_messages(),
                    ),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled("  ›  ", Style::default().fg(theme::border())),
                Span::styled(
                    config::item_label(
                        entry.item,
                        messages,
                        app.config_messages(),
                        app.guard_messages(),
                        app.job_messages(),
                        app.update_messages(),
                    ),
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {}",
                        budget_summary(budget.level, effective_output.fastctx_budget)
                    ),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                ),
            ])];
            lines.push(Line::styled(
                if budget.explicit {
                    app.config_messages().budget_explicit_note
                } else if guarded {
                    app.guard_messages().budget_follows_guarded_note
                } else {
                    app.config_messages().budget_follows_tier_note
                },
                Style::default().fg(theme::muted()),
            ));
            if !compact {
                lines.push(Line::styled(
                    budget_tool_note(messages, entry.item),
                    Style::default().fg(theme::muted()),
                ));
                if matches!(
                    entry.item,
                    ConfigItemId::RunBudget | ConfigItemId::JobOutputBudget
                ) {
                    lines.push(Line::styled(
                        messages.shell_budget_note,
                        Style::default().fg(theme::muted()),
                    ));
                }
                lines.push(Line::styled(
                    messages.budgets_note,
                    Style::default().fg(theme::muted()),
                ));
            }
            lines
        }
        ConfigValue::Toggle(enabled) => {
            let note = match entry.item {
                ConfigItemId::FastShell => messages.fastshell_note,
                ConfigItemId::UpdateAutoCheck => app.update_messages().auto_check_note,
                ConfigItemId::OutputGuard if app.output_guard_active() => {
                    app.guard_messages().active_note(app.output_guard_reason())
                }
                ConfigItemId::OutputGuard if !enabled => app.guard_messages().disabled_note,
                ConfigItemId::OutputGuard => app.guard_messages().available_note,
                _ => messages.extensions_note,
            };
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        config::item_label(
                            entry.item,
                            messages,
                            app.config_messages(),
                            app.guard_messages(),
                            app.job_messages(),
                            app.update_messages(),
                        ),
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(theme::border())),
                    Span::styled(
                        toggle_label(messages, enabled),
                        Style::default()
                            .fg(if enabled {
                                theme::success()
                            } else {
                                theme::muted()
                            })
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::raw(""),
                Line::styled(note, Style::default().fg(theme::fg())),
            ];
            if entry.item == ConfigItemId::FastShell {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    messages.extensions_note,
                    Style::default().fg(theme::muted()),
                ));
            }
            lines
        }
        ConfigValue::Number(value) => {
            let note = match entry.item {
                ConfigItemId::JobStorageLimit => app.job_messages().storage_note,
                ConfigItemId::MaxRunningJobs => app.job_messages().running_limit_note,
                ConfigItemId::JobListLimit => app.job_messages().job_list_limit_note,
                _ => "",
            };
            vec![
                Line::from(vec![
                    Span::styled(
                        config::item_label(
                            entry.item,
                            messages,
                            app.config_messages(),
                            app.guard_messages(),
                            app.job_messages(),
                            app.update_messages(),
                        ),
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(theme::border())),
                    Span::styled(
                        config_value_label(app, entry.item, ConfigValue::Number(value)),
                        Style::default()
                            .fg(theme::fg())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::raw(""),
                Line::styled(note, Style::default().fg(theme::fg())),
                Line::raw(""),
                Line::styled(
                    app.job_messages().user_limit_note,
                    Style::default().fg(theme::muted()),
                ),
            ]
        }
        ConfigValue::ReplaceLimit(value) => {
            let valid = (MIN_REPLACE_FILE_LIMIT_MIB..=MAX_REPLACE_FILE_LIMIT_MIB).contains(&value);
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        app.config_messages().replace_limit_label,
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(theme::border())),
                    Span::styled(
                        config_value_label(app, entry.item, ConfigValue::ReplaceLimit(value)),
                        Style::default()
                            .fg(config_value_color(ConfigValue::ReplaceLimit(value)))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::raw(""),
                Line::styled(
                    app.config_messages().replace_limit_note,
                    Style::default().fg(theme::fg()),
                ),
            ];
            if !valid {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    format!(
                        "replace.max_file_size_mib · {MIN_REPLACE_FILE_LIMIT_MIB}..={MAX_REPLACE_FILE_LIMIT_MIB} MiB"
                    ),
                    Style::default().fg(theme::danger()),
                ));
            }
            lines
        }
        ConfigValue::Source(source) => vec![
            Line::from(vec![
                Span::styled(
                    app.update_messages().source_label,
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ·  ", Style::default().fg(theme::border())),
                Span::styled(
                    source.as_str(),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::raw(""),
            Line::styled(
                app.update_messages().source_note,
                Style::default().fg(theme::fg()),
            ),
        ],
        ConfigValue::CpuLimit(configured) => {
            let resolution = search_parallelism::resolve(configured);
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    app.config_messages().cpu_limit_label,
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ·  ", Style::default().fg(theme::border())),
                Span::styled(
                    config_value_label(app, entry.item, ConfigValue::CpuLimit(configured)),
                    Style::default()
                        .fg(config_value_color(ConfigValue::CpuLimit(configured)))
                        .add_modifier(Modifier::BOLD),
                ),
            ])];
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                app.config_messages().cpu_limit_note,
                Style::default().fg(theme::fg()),
            ));
            lines.push(Line::raw(""));
            match resolution {
                Ok(resolved) => lines.push(Line::styled(
                    format!(
                        "search.max_cpu_cores · 1..={} · effective P={}",
                        resolved.available, resolved.effective
                    ),
                    Style::default().fg(theme::muted()),
                )),
                Err(error) => lines.push(Line::styled(
                    app.config_messages()
                        .input_error_range
                        .replace("{maximum}", &error.maximum.to_string()),
                    Style::default().fg(theme::danger()),
                )),
            }
            lines
        }
        ConfigValue::Action if entry.item == ConfigItemId::SaveAll => {
            let pending = app.config_unsaved_count();
            vec![
                Line::styled(
                    save_button_face(app, pending),
                    Style::default()
                        .fg(save_button_color(pending))
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::styled(
                    app.config_messages().save_all_note,
                    Style::default().fg(theme::fg()),
                ),
            ]
        }
        ConfigValue::Action => vec![
            Line::styled(
                app.config_messages().reset_all_label,
                Style::default()
                    .fg(theme::danger())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                app.config_messages().reset_all_note,
                Style::default().fg(theme::fg()),
            ),
        ],
    };
    let mut detail = Paragraph::new(detail).wrap(Wrap { trim: false });
    if !compact {
        // A pinned group carries no heading of its own, so the focused item names its own detail.
        let title = config::group_title(
            entry.group,
            messages,
            app.config_messages(),
            app.guard_messages(),
            app.update_messages(),
        )
        .unwrap_or_else(|| {
            config::item_label(
                entry.item,
                messages,
                app.config_messages(),
                app.guard_messages(),
                app.job_messages(),
                app.update_messages(),
            )
        });
        detail = detail.block(panel(title));
    }
    frame.render_widget(detail, chunks[2]);
}

/// What the save button says it would do, which is also how the detail pane announces it.
fn save_button_face(app: &App, pending: usize) -> String {
    let messages = app.config_messages();
    if pending == 0 {
        return messages.save_button_clean.to_string();
    }
    messages
        .save_button_dirty
        .replace("{count}", &pending.to_string())
}

fn save_button_color(pending: usize) -> Color {
    if pending == 0 {
        theme::muted()
    } else {
        theme::success()
    }
}

/// Pinned save button, the one control that ends an editing session.
///
/// It never scrolls, so it is always one keypress away from wherever the cursor is. Its face
/// reports what pressing it would do, which also answers "did anything register?" without the
/// reader scanning back up the list for unsaved markers.
fn config_save_button_line(app: &App) -> Line<'static> {
    let pending = app.config_unsaved_count();
    let focused = app.config_cursor.entry().item == ConfigItemId::SaveAll;
    let mut style = Style::default()
        .fg(save_button_color(pending))
        .add_modifier(Modifier::BOLD);
    if focused {
        style = style.bg(theme::bg_raised());
    }
    Line::from(vec![
        // The caret, not the highlight, is what survives a monochrome terminal.
        Span::styled(
            if focused { "❯ " } else { "  " },
            Style::default().fg(theme::accent()),
        ),
        // Brackets rather than a drawn box: a bracketed face reads as pressable in every terminal
        // without spending two more rows on a border.
        Span::styled(format!("[  {}  ]", save_button_face(app, pending)), style),
    ])
}

fn budget_tool_note(messages: &crate::control::i18n::Messages, item: ConfigItemId) -> &'static str {
    match item {
        ConfigItemId::ReadBudget => messages.read_tool_note,
        ConfigItemId::GrepBudget => messages.grep_tool_note,
        ConfigItemId::GlobBudget => messages.glob_tool_note,
        ConfigItemId::RunBudget => messages.run_tool_note,
        ConfigItemId::JobOutputBudget => messages.job_output_tool_note,
        _ => messages.budgets_note,
    }
}

fn render_cpu_limit_editor(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let messages = app.config_messages();
    let maximum = search_parallelism::detected_available();
    let prompt = messages
        .cpu_edit_prompt
        .replace("{maximum}", &maximum.to_string());
    let error = app.cpu_limit_editor.error.map(|error| match error {
        SearchParallelismInputError::Empty { .. } => messages.input_error_empty.to_string(),
        SearchParallelismInputError::NotInteger { .. } => {
            messages.input_error_not_integer.to_string()
        }
        SearchParallelismInputError::OutOfRange { .. } => messages
            .input_error_range
            .replace("{maximum}", &error.maximum().to_string()),
    });
    let color = if error.is_some() {
        theme::danger()
    } else {
        theme::accent()
    };
    if area.width < 72 || area.height < 12 {
        let guidance = error.unwrap_or(prompt);
        let mut lines = vec![
            Line::styled(
                messages.cpu_edit_title,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Line::from(vec![
                Span::styled(
                    app.cpu_limit_editor.input.clone(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled("▌", Style::default().fg(color)),
            ]),
            Line::styled(guidance, Style::default().fg(color)),
        ];
        if area.height >= 7 {
            lines.push(Line::styled(
                messages.cpu_limit_note,
                Style::default().fg(theme::muted()),
            ));
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            inner(area, 1, 0),
        );
        return;
    }
    let popup = centered_rect(72, 54, area);
    frame.render_widget(Clear, popup);
    let mut lines = vec![
        Line::styled(prompt, Style::default().fg(theme::fg())),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                app.cpu_limit_editor.input.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(color)),
        ])
        .alignment(Alignment::Center),
    ];
    if let Some(error) = error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(error, Style::default().fg(theme::danger())));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        messages.cpu_limit_note,
        Style::default().fg(theme::muted()),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(panel(messages.cpu_edit_title).border_style(Style::default().fg(color)))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_budget_editor(frame: &mut Frame<'_>, app: &App, area: Rect, item: ConfigItemId) {
    let messages = app.config_messages();
    let prompt = messages.budget_edit_prompt.replace(
        "{tool}",
        config::item_label(
            item,
            app.messages(),
            messages,
            app.guard_messages(),
            app.job_messages(),
            app.update_messages(),
        ),
    );
    let error = app.budget_editor.error.map(|error| match error {
        ToolBudgetInputError::Empty => messages.input_error_empty.to_string(),
        ToolBudgetInputError::NotInteger => messages.input_error_not_integer.to_string(),
        // The editable range is fixed rather than machine-derived, unlike the CPU limit.
        ToolBudgetInputError::OutOfRange => messages.input_error_range.replace("{maximum}", "100"),
    });
    // Showing what the typed share resolves to is the whole point of accepting free entry: a
    // percentage on its own says nothing about how much output it buys at the current tier.
    let preview = budget_editor::parse_input(&app.budget_editor.input)
        .ok()
        .map(|level| {
            let guarded = app.output_guard_active();
            let value = app
                .config_draft
                .preview_tool_budget_with_guard(item, Some(level), guarded);
            budget_summary(value.level, app.effective_output().fastctx_budget)
        });
    let color = if error.is_some() {
        theme::danger()
    } else {
        theme::accent()
    };
    let input_line = || {
        Line::from(vec![
            Span::styled(
                app.budget_editor.input.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(color)),
        ])
    };
    if area.width < 72 || area.height < 12 {
        let guidance = error.clone().unwrap_or(prompt);
        let mut lines = vec![
            Line::styled(
                messages.budget_edit_title,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            input_line(),
            Line::styled(guidance, Style::default().fg(color)),
        ];
        if area.height >= 7 {
            lines.push(Line::styled(
                preview.unwrap_or_else(|| messages.budget_edit_note.to_string()),
                Style::default().fg(theme::muted()),
            ));
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            inner(area, 1, 0),
        );
        return;
    }
    let popup = centered_rect(72, 54, area);
    frame.render_widget(Clear, popup);
    let mut lines = vec![
        Line::styled(prompt, Style::default().fg(theme::fg())),
        Line::raw(""),
        input_line().alignment(Alignment::Center),
    ];
    if let Some(preview) = preview {
        lines.push(
            Line::styled(preview, Style::default().fg(theme::accent()))
                .alignment(Alignment::Center),
        );
    }
    if let Some(error) = error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(error, Style::default().fg(theme::danger())));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        messages.budget_edit_note,
        Style::default().fg(theme::muted()),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(panel(messages.budget_edit_title).border_style(Style::default().fg(color)))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_jobs(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let jobs = match &app.jobs_state {
        JobsState::Loading => {
            render_loading(frame, app, area, app.job_messages().loading);
            return;
        }
        JobsState::Empty => {
            let message = format!(
                "{}\n\n{}\n\n{}",
                app.job_messages().empty,
                app.job_messages().empty_note,
                app.job_messages().history_note
            );
            render_message_panel(
                frame,
                inner(area, 2, 1),
                app.job_messages().title,
                &message,
                theme::muted(),
            );
            return;
        }
        JobsState::PermissionDenied(error) => {
            let message = format!("{error}\n\n{}", app.job_messages().error_note);
            render_message_panel(
                frame,
                inner(area, 2, 1),
                app.job_messages().permission_title,
                &message,
                theme::warning(),
            );
            return;
        }
        JobsState::Error(error) => {
            let message = format!("{error}\n\n{}", app.job_messages().error_note);
            render_message_panel(
                frame,
                inner(area, 2, 1),
                app.job_messages().error_title,
                &message,
                theme::danger(),
            );
            return;
        }
        JobsState::Ready(jobs) => jobs.clone(),
    };

    let groups = grouped_jobs(&jobs);
    let focused_job = groups
        .iter()
        .flat_map(|group| group.jobs.iter().copied())
        .nth(app.jobs_selected);
    if focused_job.is_none() {
        render_message_panel(
            frame,
            inner(area, 2, 1),
            app.job_messages().title,
            app.job_messages().history_note,
            theme::muted(),
        );
        return;
    }

    let compact = area.width < 78 || area.height < 15;
    let content = inner(area, 1, 0);
    if compact {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(44), Constraint::Min(3)])
            .split(content);
        render_job_list(frame, app, &jobs, &groups, focused_job, chunks[0]);
        render_job_output(frame, app, focused_job, chunks[1], true);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(content);
        render_job_list(frame, app, &jobs, &groups, focused_job, columns[0]);
        let detail = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(4)])
            .split(columns[1]);
        render_job_metadata(frame, app, focused_job, detail[0]);
        render_job_output(frame, app, focused_job, detail[1], false);
    }
}

enum JobDashboardRow<'a> {
    Source {
        source: &'a JobSourceSummary,
        total: usize,
    },
    Job(&'a JobSummary),
}

fn render_job_list(
    frame: &mut Frame<'_>,
    app: &mut App,
    jobs: &[JobSummary],
    groups: &[JobGroup<'_>],
    focused_job: Option<&JobSummary>,
    area: Rect,
) {
    let focused_id = focused_job.map(|job| job.id.as_str());
    let mut rows = Vec::<JobDashboardRow<'_>>::new();
    let mut focused_row = 0;
    for group in groups {
        rows.push(JobDashboardRow::Source {
            source: group.source,
            total: group.total,
        });
        for job in &group.jobs {
            if focused_id == Some(job.id.as_str()) {
                focused_row = rows.len();
            }
            rows.push(JobDashboardRow::Job(job));
        }
    }

    let visible_rows = usize::from(area.height.saturating_sub(2).max(1));
    let window = app
        .jobs_viewport
        .window(focused_row, rows.len(), visible_rows);
    let mut items = Vec::new();
    if window.show_above {
        items.push(ListItem::new(Line::styled(
            app.messages().config_more_above,
            Style::default().fg(theme::muted()),
        )));
    }
    let selected_index = focused_row.saturating_sub(window.start) + usize::from(window.show_above);
    let row_width = usize::from(area.width.saturating_sub(6));
    items.extend(rows[window.start..window.end].iter().map(|row| match row {
        JobDashboardRow::Source { source, total } => source_header_row(source, *total, row_width),
        JobDashboardRow::Job(job) => job_list_row(job, row_width),
    }));
    if window.show_below {
        items.push(ListItem::new(Line::styled(
            app.messages().config_more_below,
            Style::default().fg(theme::muted()),
        )));
    }

    let running = jobs.len();
    let summary = app
        .job_messages()
        .summary
        .replace("{running}", &running.to_string())
        .replace("{total}", &jobs.len().to_string())
        .replace("{sources}", &source_count(jobs).to_string());
    let title = format!(
        "{} · {} · {}",
        app.job_messages().footer_scope,
        app.job_messages().title,
        summary
    );
    let mut state = ListState::default().with_selected(Some(selected_index));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(&title))
            .highlight_style(selected_style())
            .highlight_symbol("❯ "),
        area,
        &mut state,
    );
}

fn source_header_row(
    source: &JobSourceSummary,
    total: usize,
    available_width: usize,
) -> ListItem<'static> {
    let workspace = path_leaf(&source.server_cwd);
    let parent = source
        .parent_executable
        .as_deref()
        .map(path_leaf)
        .filter(|name| !name.is_empty())
        .map(|name| format!(" · {name}"))
        .unwrap_or_default();
    let header = format!(
        "{workspace} · #{} · PID {}{parent}   ●{total}",
        source.tag, source.server_pid
    );
    ListItem::new(Line::styled(
        truncate_display_width(&header, available_width),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ))
}

fn job_list_row(job: &JobSummary, available_width: usize) -> ListItem<'static> {
    let now = OffsetDateTime::now_utc();
    let (age, id, command) = job_list_columns(job, available_width, now);
    ListItem::new(Line::from(vec![
        Span::styled("●  ", Style::default().fg(theme::success())),
        Span::styled(format!("{age}  "), Style::default().fg(theme::muted())),
        Span::styled(format!("{id}  "), Style::default().fg(theme::fg())),
        Span::styled(command, Style::default().fg(theme::muted())),
    ]))
}

fn job_list_columns(
    job: &JobSummary,
    available_width: usize,
    now: OffsetDateTime,
) -> (String, String, String) {
    const PREFIX_WIDTH: usize = 19;
    let age = right_align_display_width(
        &relative_started_at_at(&job.started_at, now).unwrap_or_else(|| "—".to_string()),
        4,
    );
    let id = pad_display_width(&truncate_display_width(&job.id, 8), 8);
    let command_width = available_width.saturating_sub(PREFIX_WIDTH);
    let command = truncate_display_width(&escape_controls(&job.command), command_width);
    (age, id, command)
}

fn render_job_metadata(
    frame: &mut Frame<'_>,
    app: &App,
    focused_job: Option<&JobSummary>,
    area: Rect,
) {
    let Some(job) = focused_job else {
        return;
    };
    let status = app.job_messages().status_running;
    let color = theme::success();
    let now = OffsetDateTime::now_utc();
    let started_at = exact_started_at(&job.started_at).unwrap_or_else(|| "—".to_string());
    let elapsed = elapsed_hms_at(&job.started_at, now).unwrap_or_else(|| "—".to_string());
    let workspace = path_leaf(&job.source.server_cwd);
    let parent = job
        .source
        .parent_executable
        .as_deref()
        .map(path_leaf)
        .filter(|value| !value.is_empty())
        .map(|value| format!(" · {value}"))
        .unwrap_or_default();
    let lines = vec![
        Line::from(vec![
            Span::styled("●  ", Style::default().fg(color)),
            Span::styled(
                status,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {} {elapsed}", app.job_messages().elapsed_label),
                Style::default().fg(theme::muted()),
            ),
        ]),
        Line::from(vec![
            Span::styled("◷  ", Style::default().fg(theme::muted())),
            Span::styled(
                format!("{} {started_at}", app.job_messages().started_label),
                Style::default().fg(theme::fg()),
            ),
        ]),
        Line::from(vec![
            Span::styled("⌂  ", Style::default().fg(theme::muted())),
            Span::styled(escape_controls(&job.cwd), Style::default().fg(theme::fg())),
        ]),
        Line::from(vec![
            Span::styled("$  ", Style::default().fg(theme::muted())),
            Span::styled(
                escape_controls(&job.command),
                Style::default().fg(theme::fg()),
            ),
        ]),
        Line::from(vec![
            Span::styled("◇  ", Style::default().fg(theme::muted())),
            Span::styled(
                format!(
                    "{workspace} · #{} · PID {}{parent}",
                    job.source.tag, job.source.server_pid
                ),
                Style::default().fg(theme::muted()),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).block(panel(&job.id)), area);
}

fn render_job_output(
    frame: &mut Frame<'_>,
    app: &App,
    focused_job: Option<&JobSummary>,
    area: Rect,
    compact: bool,
) {
    let Some(job) = focused_job else {
        render_message_panel(
            frame,
            area,
            app.job_messages().output_title,
            app.job_messages().output_empty,
            theme::muted(),
        );
        return;
    };
    let follow = if app.jobs_detail.follow_tail {
        app.job_messages().follow_on
    } else {
        app.job_messages().follow_off
    };
    let content_width = usize::from(area.width.saturating_sub(4).max(1));
    let has_right = app.jobs_detail.tail.lines.iter().any(|line| {
        Line::from(display_output_line(line)).width()
            > app.jobs_detail.horizontal_offset + content_width
    });
    let horizontal_marker = match (app.jobs_detail.horizontal_offset > 0, has_right) {
        (true, true) => "←→",
        (true, false) => "←",
        (false, true) => "→",
        (false, false) => "",
    };
    let elapsed = elapsed_hms_at(&job.started_at, OffsetDateTime::now_utc())
        .unwrap_or_else(|| "—".to_string());
    let detail_title = if compact {
        let started = exact_started_time(&job.started_at).unwrap_or_else(|| "—".to_string());
        if horizontal_marker.is_empty() {
            format!("{started} · {elapsed} · {}", job.id)
        } else {
            format!(
                "{started} · {elapsed} · {} · {horizontal_marker} @{}",
                job.id,
                app.jobs_detail.horizontal_offset + 1
            )
        }
    } else if horizontal_marker.is_empty() {
        format!(
            "{} · {} · {} · {}",
            app.job_messages().output_title,
            job.id,
            elapsed,
            follow
        )
    } else {
        format!(
            "{} · {} · {} · {} · {} @{}",
            app.job_messages().output_title,
            job.id,
            elapsed,
            follow,
            horizontal_marker,
            app.jobs_detail.horizontal_offset + 1
        )
    };
    let detail_matches = app.jobs_detail.job_id.as_deref() == Some(job.id.as_str());
    let mut lines = Vec::new();
    if !detail_matches {
        lines.push(Line::styled(
            app.job_messages().loading,
            Style::default().fg(theme::muted()),
        ));
    } else if let Some(error) = &app.jobs_detail.error {
        lines.push(Line::styled(error, Style::default().fg(theme::danger())));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            app.job_messages().error_note,
            Style::default().fg(theme::muted()),
        ));
    } else {
        if let Some(error) = &app.jobs_detail.tail.capture_error {
            lines.push(Line::styled(error, Style::default().fg(theme::danger())));
            lines.push(Line::raw(""));
        }
        if let Some(warning) = &app.jobs_detail.tail.output_truncation {
            lines.push(Line::styled(warning, Style::default().fg(theme::warning())));
            lines.push(Line::raw(""));
        }
        let output = &app.jobs_detail.tail.lines;
        if output.is_empty() {
            lines.push(Line::styled(
                app.job_messages().output_empty,
                Style::default().fg(theme::muted()),
            ));
        } else {
            let available = usize::from(area.height.saturating_sub(2).max(1));
            let max_scroll = output.len().saturating_sub(available);
            let scroll = app.jobs_detail.lines_below.min(max_scroll);
            let end = output.len().saturating_sub(scroll);
            let start = end.saturating_sub(available);
            lines.extend(output[start..end].iter().map(|line| {
                Line::styled(display_output_line(line), Style::default().fg(theme::fg()))
            }));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(&detail_title)).scroll((
            0,
            u16::try_from(app.jobs_detail.horizontal_offset).unwrap_or(u16::MAX),
        )),
        area,
    );
}

fn render_job_kill_confirmation(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let popup = centered_rect(72, 54, area);
    frame.render_widget(Clear, popup);
    let no_style = if app.selected == 0 {
        Style::default()
            .fg(theme::bg())
            .bg(theme::muted())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::muted())
    };
    let yes_style = if app.selected == 1 {
        Style::default()
            .fg(theme::bg())
            .bg(theme::danger())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::danger())
    };
    let (job_id, command) = app.pending_job.as_ref().map_or_else(
        || ("—".to_string(), "—".to_string()),
        |job| {
            (
                job.id.clone(),
                truncate_end(&escape_controls(&job.command), 120),
            )
        },
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                job_id,
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
            Line::raw(""),
            Line::styled(command, Style::default().fg(theme::muted())).alignment(Alignment::Center),
            Line::raw(""),
            Line::styled(
                app.job_messages().kill_warning,
                Style::default()
                    .fg(theme::danger())
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center),
            Line::raw(""),
            Line::from(vec![
                Span::styled("  ✕  ", no_style),
                Span::raw("     "),
                Span::styled("  ✓  ", yes_style),
            ])
            .alignment(Alignment::Center),
        ])
        .alignment(Alignment::Center)
        .block(
            panel(app.job_messages().kill_prompt)
                .border_style(Style::default().fg(theme::danger())),
        )
        .wrap(Wrap { trim: false }),
        popup,
    );
}

fn path_leaf(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn relative_started_at_at(started_at: &str, now: OffsetDateTime) -> Option<String> {
    let seconds = OffsetDateTime::parse(started_at, &Rfc3339)
        .ok()
        .map(|started| (now - started).whole_seconds().max(0))?;
    if seconds < 60 {
        Some(format!("{seconds}s"))
    } else if seconds < 3_600 {
        Some(format!("{}m", seconds / 60))
    } else if seconds < 86_400 {
        Some(format!("{}h", seconds / 3_600))
    } else {
        Some(format!("{}d", seconds / 86_400))
    }
}

fn exact_started_at(started_at: &str) -> Option<String> {
    let started = OffsetDateTime::parse(started_at, &Rfc3339)
        .ok()?
        .to_offset(time::UtcOffset::UTC);
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        started.year(),
        u8::from(started.month()),
        started.day(),
        started.hour(),
        started.minute(),
        started.second()
    ))
}

fn exact_started_time(started_at: &str) -> Option<String> {
    let started = OffsetDateTime::parse(started_at, &Rfc3339)
        .ok()?
        .to_offset(time::UtcOffset::UTC);
    Some(format!(
        "{:02}:{:02}:{:02} UTC",
        started.hour(),
        started.minute(),
        started.second()
    ))
}

fn elapsed_hms_at(started_at: &str, now: OffsetDateTime) -> Option<String> {
    let started = OffsetDateTime::parse(started_at, &Rfc3339).ok()?;
    let seconds = (now - started).whole_seconds().max(0);
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    Some(format!("{hours:02}:{minutes:02}:{seconds:02}"))
}

fn truncate_display_width(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if Line::from(value).width() <= max_width {
        return value.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let content_width = max_width - 1;
    let mut used = 0_usize;
    let mut result = String::new();
    for character in value.chars() {
        let width = Line::from(character.to_string()).width();
        if used.saturating_add(width) > content_width {
            break;
        }
        result.push(character);
        used = used.saturating_add(width);
    }
    result.push_str(&" ".repeat(content_width.saturating_sub(used)));
    result.push('…');
    result
}

fn pad_display_width(value: &str, width: usize) -> String {
    let missing = width.saturating_sub(Line::from(value).width());
    format!("{value}{}", " ".repeat(missing))
}

fn right_align_display_width(value: &str, width: usize) -> String {
    let value = truncate_display_width(value, width);
    let missing = width.saturating_sub(Line::from(value.as_str()).width());
    format!("{}{value}", " ".repeat(missing))
}

fn escape_controls(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut characters = value.chars();
    let prefix = characters
        .by_ref()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        value.to_string()
    }
}

fn config_more_row(label: &'static str) -> Row<'static> {
    let mut parts = label.splitn(2, char::is_whitespace);
    let arrow = parts.next().unwrap_or(label);
    let text = parts.next().unwrap_or_default();
    Row::new(vec![
        Cell::from(Line::styled(arrow, Style::default().fg(theme::muted()))),
        Cell::from(Line::styled(text, Style::default().fg(theme::muted()))),
    ])
}

fn config_item_row(
    app: &App,
    group: ConfigGroupId,
    item: ConfigItemId,
    role: ConfigItemRole,
) -> Row<'static> {
    let entry = app.config_cursor.entry();
    let selected = entry.group == group && entry.item == item;
    let marker = if selected { "❯ " } else { "  " };
    let hierarchy = match role {
        ConfigItemRole::Parent => "  ",
        ConfigItemRole::Child { is_last: false } => "    ├─ ",
        ConfigItemRole::Child { is_last: true } => "    └─ ",
    };
    let value_line = config_value_line(app, item, selected);
    let base = if selected {
        Style::default().bg(theme::bg_raised())
    } else {
        Style::default()
    };
    Row::new(vec![
        Cell::from(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme::accent())),
            Span::styled(hierarchy, Style::default().fg(theme::border())),
            Span::styled(
                config::item_label(
                    item,
                    app.messages(),
                    app.config_messages(),
                    app.guard_messages(),
                    app.job_messages(),
                    app.update_messages(),
                )
                .to_string(),
                if selected {
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::muted())
                },
            ),
        ])),
        Cell::from(value_line),
    ])
    .style(base)
}

/// The value column for one row, followed by the unsaved marker when the drafted value has not
/// reached disk yet. The marker is a glyph rather than a colour so it survives monochrome
/// terminals, where colour alone would erase the distinction.
fn config_value_line(app: &App, item: ConfigItemId, selected: bool) -> Line<'static> {
    let value = app
        .config_draft
        .value_with_guard(item, app.output_guard_active());
    let mut line = if value == ConfigValue::Action {
        Line::styled(
            format!("Enter · {}", app.config_messages().reset_all_label),
            Style::default()
                .fg(theme::danger())
                .add_modifier(Modifier::BOLD),
        )
    } else if item == ConfigItemId::OutputGuard {
        Line::styled(
            format!("Enter · {}", config_value_label(app, item, value)),
            Style::default()
                .fg(config_value_color(value))
                .add_modifier(Modifier::BOLD),
        )
    } else if matches!(value, ConfigValue::GuardedTier(_)) {
        Line::styled(
            "Guarded",
            Style::default()
                .fg(theme::warning())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Line::from(vec![
            Span::styled(
                "‹ ",
                Style::default().fg(if selected {
                    theme::accent()
                } else {
                    theme::border()
                }),
            ),
            Span::styled(
                config_value_label(app, item, value),
                Style::default()
                    .fg(config_value_color(value))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ›",
                Style::default().fg(if selected {
                    theme::accent()
                } else {
                    theme::border()
                }),
            ),
        ])
    };
    if app
        .config_draft
        .item_changed(app.saved_config_draft(), item, app.output_guard_active())
    {
        line.spans
            .push(Span::styled(" ●", Style::default().fg(theme::warning())));
    }
    line
}

fn render_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    match &app.status {
        StatusState::Loading => render_loading(frame, app, area, app.messages().loading),
        StatusState::Empty => render_message_panel(
            frame,
            inner(area, 3, 2),
            app.messages().status_title,
            app.messages().empty,
            theme::muted(),
        ),
        StatusState::Error(error) => render_message_panel(
            frame,
            area,
            app.messages().status_title,
            &format!("{error}\n\n{}", app.messages().action_retry),
            theme::danger(),
        ),
        StatusState::Ready(report) => {
            let rows = report.checks.iter().map(|check| {
                let (mark, color) = match check.status {
                    DoctorCheckStatus::Pass => ("✓", theme::success()),
                    DoctorCheckStatus::Info => ("○", theme::muted()),
                    DoctorCheckStatus::Fail => ("×", theme::danger()),
                };
                let height = if check.remedy.is_some() { 2 } else { 1 };
                let detail = if let Some(remedy) = &check.remedy {
                    format!("{}\n{remedy}", check.detail)
                } else {
                    check.detail.clone()
                };
                Row::new(vec![
                    Cell::from(Span::styled(
                        mark,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )),
                    Cell::from(localized_check_name(app, check.name)),
                    Cell::from(detail),
                ])
                .height(height)
            });
            frame.render_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Length(3),
                        Constraint::Length(20),
                        Constraint::Min(20),
                    ],
                )
                .column_spacing(1)
                .block(panel(app.messages().status_title)),
                inner(area, 1, 1),
            );
        }
    }
}

fn render_about(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = vec![
        Line::styled(
            "FastCtx",
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme::muted()),
        ),
        Line::raw(""),
        Line::styled(
            "FastCtx — fast, context-efficient repository tools for AI agents.",
            Style::default().fg(theme::fg()),
        ),
        Line::raw(""),
        Line::styled(
            "https://github.com/yc-duan/fastctx",
            Style::default().fg(theme::muted()),
        ),
        Line::styled(
            "https://github.com/yc-duan/fastctx/issues",
            Style::default().fg(theme::muted()),
        ),
        Line::styled("Apache-2.0", Style::default().fg(theme::muted())),
        Line::styled(
            "Copyright (c) 2026 yc-duan <dy2958830371@gmail.com>",
            Style::default().fg(theme::muted()),
        ),
        Line::raw(""),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(panel(app.messages().about_title))
            .wrap(Wrap { trim: false }),
        inner(area, 3, 2),
    );
}

fn render_receipt(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if let Some(receipt) = &app.receipt {
        lines.push(Line::styled(
            format!("✓ {}", receipt.changed_targets),
            Style::default()
                .fg(theme::success())
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        for note in &receipt.notes {
            if note == "No changes were needed." {
                lines.push(Line::raw(app.messages().no_changes));
            } else if note != "Changes apply to newly started ChatGPT/Codex sessions." {
                lines.push(Line::raw(note));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            app.messages().restart_notice,
            Style::default().fg(theme::accent()),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app.messages().receipt_title))
            .wrap(Wrap { trim: false }),
        inner(area, 3, 2),
    );
}

fn render_error(frame: &mut Frame<'_>, app: &App, area: Rect) {
    render_message_panel(
        frame,
        inner(area, 3, 2),
        app.messages().operation_failed,
        &format!(
            "{}\n\n{}",
            app.error
                .as_deref()
                .unwrap_or(app.messages().operation_failed),
            app.messages().action_retry
        ),
        theme::danger(),
    );
}

fn render_confirmation(frame: &mut Frame<'_>, app: &App, area: Rect, prompt: &str, color: Color) {
    let panel_title = prompt.lines().next().unwrap_or(prompt);
    let no_style = if app.selected == 0 {
        Style::default()
            .fg(theme::bg())
            .bg(theme::muted())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::muted())
    };
    let yes_style = if app.selected == 1 {
        Style::default()
            .fg(theme::bg())
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    let actions = Line::from(vec![
        Span::styled("  ✕  ", no_style),
        Span::raw("     "),
        Span::styled("  ✓  ", yes_style),
    ])
    .alignment(Alignment::Center);
    if area.width < 72 || area.height < 14 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner(area, 1, 0));
        frame.render_widget(
            Paragraph::new(prompt)
                .alignment(Alignment::Center)
                .style(
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                )
                .wrap(Wrap { trim: false }),
            rows[0],
        );
        frame.render_widget(Paragraph::new(actions), rows[1]);
        return;
    }
    let popup = centered_rect(66, 38, area);
    frame.render_widget(Clear, popup);
    // A Line cannot hold a newline, so the prompt has to be split by hand or its paragraphs run
    // together. The panel title already carries the first line, so the body starts after it
    // unless the prompt is that one line.
    let mut body = prompt
        .lines()
        .skip(usize::from(prompt.lines().nth(1).is_some()))
        .map(|line| {
            Line::styled(
                line.to_string(),
                Style::default()
                    .fg(theme::fg())
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    body.push(Line::raw(""));
    body.push(actions);
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .block(panel(panel_title).border_style(Style::default().fg(color)))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_loading(frame: &mut Frame<'_>, app: &App, area: Rect, message: &str) {
    frame.render_widget(
        Paragraph::new(Line::styled(
            message,
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center)
        .block(panel(app.messages().app_title)),
        inner(area, 5, 3),
    );
}

fn render_migration_notice(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let messages = app.migration_messages();
    let body = messages
        .body
        .replace("{version}", env!("CARGO_PKG_VERSION"));
    let horizontal_margin = if area.width >= 64 { 4 } else { 0 };
    let vertical_margin = if area.height >= 10 { 1 } else { 0 };
    let panel_area = inner(area, horizontal_margin, vertical_margin);
    let content_area = inner(panel_area, 2, 1);
    let content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(content_area);
    let body_area = Rect {
        height: if content[0].height <= 2 {
            1
        } else {
            content[0].height
        },
        ..content[0]
    };
    let wrap_width = body_area
        .width
        .saturating_sub(if body_area.width < 50 { 10 } else { 2 });
    let body_lines = wrap_detail_lines(vec![Line::from(body)], wrap_width);
    app.detail_viewport
        .update(body_lines.len(), usize::from(body_area.height));
    let scroll_indicator = match (
        app.detail_viewport.can_move_up(),
        app.detail_viewport.can_move_down(),
    ) {
        (true, true) => " ↑↓",
        (true, false) => " ↑",
        (false, true) => " ↓",
        (false, false) => "",
    };
    let title = format!("{}{scroll_indicator}", messages.title);
    frame.render_widget(
        panel(&title).border_style(Style::default().fg(theme::accent())),
        panel_area,
    );
    frame.render_widget(
        Paragraph::new(
            body_lines
                .into_iter()
                .skip(app.detail_viewport.offset())
                .take(usize::from(body_area.height))
                .collect::<Vec<_>>(),
        )
        .style(Style::default().fg(theme::fg())),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(format!("✓ {}", messages.action_confirm))
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme::success())),
        content[1],
    );
}

fn render_message_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    message: &str,
    color: Color,
) {
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(color))
            .block(panel(title).border_style(Style::default().fg(color)))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn narrow_title(value: impl Into<String>) -> Line<'static> {
    Line::styled(
        value.into(),
        Style::default()
            .fg(theme::fg())
            .add_modifier(Modifier::BOLD),
    )
}

fn wrap_detail_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let maximum_width = usize::from(width.max(1));
    let mut wrapped = Vec::new();
    for line in lines {
        let alignment = line.alignment;
        let mut spans = Vec::new();
        let mut row_width = 0_usize;
        let mut had_grapheme = false;
        for grapheme in line.styled_graphemes(Style::default()) {
            had_grapheme = true;
            let grapheme_width = Span::raw(grapheme.symbol).width();
            if row_width > 0 && row_width.saturating_add(grapheme_width) > maximum_width {
                let mut row = Line::from(std::mem::take(&mut spans));
                row.alignment = alignment;
                wrapped.push(row);
                row_width = 0;
            }
            spans.push(Span::styled(grapheme.symbol.to_string(), grapheme.style));
            row_width = row_width.saturating_add(grapheme_width);
        }
        if !spans.is_empty() || !had_grapheme {
            let mut row = Line::from(spans);
            row.alignment = alignment;
            wrapped.push(row);
        }
    }
    wrapped
}

fn render_narrow_details(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    mut lines: Vec<Line<'static>>,
) {
    let title = if lines.is_empty() {
        Line::raw("")
    } else {
        lines.remove(0)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let lines = wrap_detail_lines(lines, chunks[1].width);
    app.detail_viewport
        .update(lines.len(), usize::from(chunks[1].height));
    let offset = app.detail_viewport.offset();
    frame.render_widget(Paragraph::new(title), chunks[0]);
    let indicator = match (
        app.detail_viewport.can_move_up(),
        app.detail_viewport.can_move_down(),
    ) {
        (true, true) => "↑↓",
        (true, false) => "↑",
        (false, true) => "↓",
        (false, false) => "",
    };
    if !indicator.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(indicator, Style::default().fg(theme::muted())))
                .alignment(Alignment::Right),
            chunks[0],
        );
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Left)
            .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        chunks[1],
    );
}

fn render_narrow_preview(frame: &mut Frame<'_>, app: &mut App, area: Rect, apply: bool) {
    let running_processes = (!apply)
        .then(|| {
            app.unapply_plan
                .as_ref()
                .map(|plan| plan.running_processes())
        })
        .flatten();
    let title = running_processes.map_or_else(
        || app.messages().preview_title.to_string(),
        |count| {
            format!(
                "{} · {}",
                app.messages().preview_title,
                app.unapply_processes_message()
                    .replace("{count}", &count.to_string())
            )
        },
    );
    let mut lines = vec![narrow_title(truncate_display_width(
        &title,
        usize::from(area.width),
    ))];
    let items = if apply {
        app.apply_plan.as_ref().map(|plan| plan.preview())
    } else {
        app.unapply_plan.as_ref().map(|plan| plan.preview())
    };
    let Some(items) = items else {
        lines.push(Line::styled(
            app.messages().loading,
            Style::default().fg(theme::muted()),
        ));
        render_narrow_details(frame, app, area, lines);
        return;
    };
    let has_changes = items
        .iter()
        .any(|item| item.action != PreviewAction::Unchanged)
        || running_processes.is_some_and(|count| count > 0);
    if !has_changes {
        lines.push(Line::styled(
            app.messages().no_changes,
            Style::default().fg(theme::success()),
        ));
        lines.push(Line::raw(""));
    }
    for item in items {
        push_preview_card(&mut lines, app, item);
        lines.push(Line::raw(""));
    }
    if let Some(count) = running_processes {
        let changed = count > 0;
        let color = if changed {
            theme::danger()
        } else {
            theme::muted()
        };
        lines.push(Line::styled(
            app.unapply_processes_message()
                .replace("{count}", &count.to_string()),
            Style::default().fg(color).add_modifier(if changed {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ));
    }
    if has_changes {
        lines.push(Line::styled(
            app.messages().restart_notice,
            Style::default().fg(theme::muted()),
        ));
    }
    render_narrow_details(frame, app, area, lines);
}

fn render_narrow_status(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let mut lines = vec![narrow_title(app.messages().status_title)];
    match &app.status {
        StatusState::Loading => lines.push(Line::styled(
            app.messages().loading,
            Style::default().fg(theme::accent()),
        )),
        StatusState::Empty => lines.push(Line::styled(
            app.messages().empty,
            Style::default().fg(theme::muted()),
        )),
        StatusState::Error(error) => {
            lines.push(Line::styled(
                truncate_display_width(error, usize::from(area.width)),
                Style::default().fg(theme::danger()),
            ));
            lines.push(Line::styled(
                app.messages().action_retry,
                Style::default().fg(theme::accent()),
            ));
        }
        StatusState::Ready(report) => {
            let count = |status| {
                report
                    .checks
                    .iter()
                    .filter(|check| check.status == status)
                    .count()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("✓ {}", count(DoctorCheckStatus::Pass)),
                    Style::default().fg(theme::success()),
                ),
                Span::raw("   "),
                Span::styled(
                    format!("○ {}", count(DoctorCheckStatus::Info)),
                    Style::default().fg(theme::muted()),
                ),
                Span::raw("   "),
                Span::styled(
                    format!("× {}", count(DoctorCheckStatus::Fail)),
                    Style::default().fg(theme::danger()),
                ),
            ]));
            for status in [
                DoctorCheckStatus::Fail,
                DoctorCheckStatus::Info,
                DoctorCheckStatus::Pass,
            ] {
                for check in report.checks.iter().filter(|check| check.status == status) {
                    let (marker, color) = match status {
                        DoctorCheckStatus::Pass => ("✓", theme::success()),
                        DoctorCheckStatus::Info => ("○", theme::muted()),
                        DoctorCheckStatus::Fail => ("×", theme::danger()),
                    };
                    let remedy = check
                        .remedy
                        .as_deref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default();
                    let summary = format!(
                        "{marker} {}: {}{remedy}",
                        localized_check_name(app, check.name),
                        check.detail
                    );
                    lines.push(Line::styled(
                        truncate_display_width(&summary, usize::from(area.width)),
                        Style::default().fg(color),
                    ));
                }
            }
        }
    }
    render_narrow_details(frame, app, area, lines);
}

fn render_narrow_receipt(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let mut lines = vec![narrow_title(app.messages().receipt_title)];
    let Some(receipt) = &app.receipt else {
        lines.push(Line::styled(
            app.messages().empty,
            Style::default().fg(theme::muted()),
        ));
        render_narrow_details(frame, app, area, lines);
        return;
    };
    lines.push(Line::styled(
        format!("✓ {}", receipt.changed_targets),
        Style::default()
            .fg(theme::success())
            .add_modifier(Modifier::BOLD),
    ));
    for note in receipt
        .notes
        .iter()
        .filter(|note| note.as_str() != "Changes apply to newly started ChatGPT/Codex sessions.")
    {
        let note = if note == "No changes were needed." {
            app.messages().no_changes
        } else {
            note
        };
        lines.push(Line::styled(
            truncate_display_width(note, usize::from(area.width)),
            Style::default().fg(theme::fg()),
        ));
    }
    lines.push(Line::styled(
        truncate_display_width(app.messages().restart_notice, usize::from(area.width)),
        Style::default().fg(theme::accent()),
    ));
    render_narrow_details(frame, app, area, lines);
}

fn render_narrow_error(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let mut lines = vec![narrow_title(app.messages().operation_failed)];
    let detail = app
        .error
        .as_deref()
        .unwrap_or(app.messages().operation_failed);
    for line in detail.lines().filter(|line| !line.is_empty()) {
        lines.push(Line::styled(
            truncate_display_width(line, usize::from(area.width)),
            Style::default().fg(theme::danger()),
        ));
    }
    lines.push(Line::styled(
        app.messages().action_retry,
        Style::default().fg(theme::accent()),
    ));
    render_narrow_details(frame, app, area, lines);
}

fn render_narrow_about(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    render_narrow_details(
        frame,
        app,
        area,
        vec![
            narrow_title(app.messages().about_title),
            Line::styled(
                format!("FastCtx v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(theme::accent()),
            ),
            Line::styled(
                "https://github.com/yc-duan/fastctx",
                Style::default().fg(theme::fg()),
            ),
            Line::styled(
                "https://github.com/yc-duan/fastctx/issues",
                Style::default().fg(theme::muted()),
            ),
            Line::styled("Apache-2.0", Style::default().fg(theme::muted())),
        ],
    );
}

fn render_narrow(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    match app.screen {
        Screen::ApplyPreview => {
            render_narrow_preview(frame, app, area, true);
            return;
        }
        Screen::UnapplyPreview => {
            render_narrow_preview(frame, app, area, false);
            return;
        }
        Screen::Status => {
            render_narrow_status(frame, app, area);
            return;
        }
        Screen::Receipt => {
            render_narrow_receipt(frame, app, area);
            return;
        }
        Screen::OperationFailed | Screen::JobsKillFailed => {
            render_narrow_error(frame, app, area);
            return;
        }
        Screen::About => {
            render_narrow_about(frame, app, area);
            return;
        }
        _ => {}
    }
    let messages = app.messages();
    let mut lines = Vec::new();
    let selected = match app.screen {
        Screen::MigrationNotice => app.migration_messages().title.to_string(),
        Screen::Update => update_state_summary(app),
        Screen::UpdateChecking => app.update_messages().checking.to_string(),
        Screen::UpdateConfirm => app.update_messages().available_title.to_string(),
        Screen::Language { .. } => format!(
            "{} · {}",
            ALL_LANGUAGES[app.selected].code(),
            ALL_LANGUAGES[app.selected].native_name()
        ),
        Screen::Main => {
            let labels = [
                messages.menu_apply,
                messages.menu_config,
                app.job_messages().menu,
                app.update_messages().page_title,
                messages.menu_status,
                messages.menu_about,
                messages.menu_language,
            ];
            main_menu_label(app, app.selected, labels[app.selected])
        }
        Screen::Connections => {
            ["ChatGPT / Codex", "DeepSeek Harness", "Disconnect all"][app.selected].to_string()
        }
        Screen::ApplyHome => {
            [messages.action_apply, messages.action_unapply, "Doctor"][app.selected].to_string()
        }
        Screen::Config => config_narrow_summary(app),
        Screen::ConfigCpuEdit => format!(
            "{} · {}",
            app.config_messages().cpu_edit_title,
            app.cpu_limit_editor.input
        ),
        Screen::ConfigBudgetEdit(_) => format!(
            "{} · {}",
            app.config_messages().budget_edit_title,
            app.budget_editor.input
        ),
        Screen::ConfigResetConfirm => app.config_messages().reset_confirm.to_string(),
        Screen::ConfigDiscardConfirm => app.config_messages().discard_confirm.to_string(),
        Screen::ConfigOutputGuardConfirm => app.guard_messages().disable_confirm.to_string(),
        Screen::Jobs => app
            .focused_job()
            .map(|job| {
                format!(
                    "{} · {}",
                    job.id,
                    truncate_end(&escape_controls(&job.command), 24)
                )
            })
            .unwrap_or_else(|| app.job_messages().empty.to_string()),
        Screen::JobsKillConfirm => app.job_messages().kill_prompt.to_string(),
        Screen::ApplyConflict => messages.conflict_warning.to_string(),
        Screen::ApplyConfirm => messages.confirm_apply.to_string(),
        Screen::UnapplyConfirm => messages.confirm_unapply.to_string(),
        Screen::ApplyLoading
        | Screen::ApplyRunning
        | Screen::UnapplyLoading
        | Screen::UnapplyRunning
        | Screen::ConfigResetting
        | Screen::ConfigSaving
        | Screen::JobsKilling => messages.loading.to_string(),
        Screen::ApplyPreview
        | Screen::UnapplyPreview
        | Screen::Status
        | Screen::About
        | Screen::Receipt
        | Screen::OperationFailed
        | Screen::JobsKillFailed => unreachable!("detail screens return before compact selection"),
    };
    lines.push(Line::styled(
        selected,
        Style::default()
            .fg(
                if app.screen == Screen::Main
                    && app.selected == 0
                    && app.link_state.requires_apply()
                {
                    theme::warning()
                } else {
                    theme::fg()
                },
            )
            .add_modifier(Modifier::BOLD),
    ));
    // A terminal too small for the panel still has to show whether the host is connected; the
    // symbol keeps it readable after the text is truncated.
    if matches!(
        app.screen,
        Screen::Main | Screen::Connections | Screen::ApplyHome
    ) {
        let status = if app.screen == Screen::Connections && app.selected == 1 {
            let state = app
                .dsh_status
                .as_ref()
                .map(|(state, _)| state.as_str())
                .unwrap_or("unhealthy");
            Line::styled(
                format!("DeepSeek Harness: {state}"),
                Style::default().fg(theme::accent()),
            )
        } else if app.screen == Screen::Connections && app.selected == 2 {
            Line::styled("Destructive action", Style::default().fg(theme::danger()))
        } else {
            link_status_line(app)
        };
        let text = status
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        lines.push(Line::styled(
            truncate_end(&text, usize::from(area.width.saturating_sub(4))),
            status.style,
        ));
        if app.link_state.requires_apply() {
            lines.push(Line::styled(
                truncate_end(
                    app.messages().link_state_stale_hint,
                    usize::from(area.width.saturating_sub(4)),
                ),
                Style::default().fg(theme::warning()),
            ));
        }
    }
    if app.screen == Screen::Update {
        let detail = match &app.update_state {
            StartupUpdate::Available(plan) => format!(
                "v{} → v{} · {}",
                env!("CARGO_PKG_VERSION"),
                plan.target_version(),
                plan.source_label()
            ),
            StartupUpdate::NpmPending { target_version, .. } => {
                format!("v{target_version} · propagation pending")
            }
            StartupUpdate::NpmCurrent { discovery } => {
                format!("v{} · current", discovery.target_version)
            }
            StartupUpdate::Failed(error) => error.message.clone(),
            _ => String::new(),
        };
        lines.push(Line::styled(
            truncate_end(&detail, usize::from(area.width.saturating_sub(4))),
            Style::default().fg(theme::muted()),
        ));
        let primary = if matches!(app.update_state, StartupUpdate::Available(_)) {
            app.update_messages().action_update
        } else {
            app.update_messages().action_check
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {primary} "),
                if app.selected == 0 {
                    Style::default().fg(theme::bg()).bg(theme::accent())
                } else {
                    Style::default().fg(theme::accent())
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!(" {} ", app.update_messages().action_continue),
                if app.selected == 1 {
                    Style::default().fg(theme::bg()).bg(theme::muted())
                } else {
                    Style::default().fg(theme::muted())
                },
            ),
        ]));
    } else if matches!(
        app.screen,
        Screen::ApplyConflict
            | Screen::ApplyConfirm
            | Screen::UnapplyConfirm
            | Screen::ConfigResetConfirm
            | Screen::ConfigDiscardConfirm
            | Screen::JobsKillConfirm
    ) {
        lines.push(Line::from(vec![
            Span::styled(
                "  ✕  ",
                if app.selected == 0 {
                    Style::default().fg(theme::bg()).bg(theme::muted())
                } else {
                    Style::default().fg(theme::muted())
                },
            ),
            Span::raw("  "),
            Span::styled(
                "  ✓  ",
                if app.selected == 1 {
                    Style::default().fg(theme::bg()).bg(theme::accent())
                } else {
                    Style::default().fg(theme::accent())
                },
            ),
        ]));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let messages = app.messages();
    if app.screen == Screen::Jobs {
        let row = jobs_footer(app, usize::from(area.width.saturating_sub(2)));
        frame.render_widget(
            Paragraph::new(Line::styled(row, Style::default().fg(theme::muted())))
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme::muted())),
            area,
        );
        return;
    }
    let hints = match app.screen {
        Screen::MigrationNotice => vec![app.migration_messages().footer_confirm],
        Screen::Update => vec![
            messages.footer_move,
            messages.footer_select,
            app.update_messages().action_check,
            messages.footer_back,
        ],
        Screen::UpdateChecking => vec![app.update_messages().checking],
        Screen::Main => vec![
            messages.footer_move,
            messages.footer_select,
            messages.footer_quit,
        ],
        Screen::ApplyLoading
        | Screen::ApplyRunning
        | Screen::UnapplyLoading
        | Screen::UnapplyRunning => vec![messages.loading],
        Screen::Language { first_run: true } => vec![messages.footer_move, messages.footer_select],
        Screen::Status => vec![
            messages.footer_move,
            messages.action_refresh,
            app.update_messages().action_check,
            messages.footer_back,
        ],
        Screen::OperationFailed | Screen::JobsKillFailed => {
            vec![
                messages.footer_move,
                messages.action_retry,
                messages.footer_back,
            ]
        }
        Screen::Jobs => unreachable!("the Jobs footer is rendered above"),
        Screen::JobsKillConfirm => vec![
            messages.footer_move,
            messages.footer_select,
            messages.footer_back,
        ],
        Screen::JobsKilling => vec![messages.loading],
        Screen::ConfigResetting => vec![messages.loading],
        // No key is offered: the write blocks this thread, so the only honest footer is the one
        // that names what is happening rather than an action nobody could take.
        Screen::ConfigSaving => vec![app.config_messages().saving_notice],
        Screen::ConfigCpuEdit | Screen::ConfigBudgetEdit(_) => {
            vec![app.config_messages().footer_accept, messages.footer_cancel]
        }
        Screen::ConfigResetConfirm
        | Screen::ConfigOutputGuardConfirm
        | Screen::ConfigDiscardConfirm => vec![
            messages.footer_move,
            messages.footer_select,
            messages.footer_back,
        ],
        Screen::Config => {
            let mut hints = vec![messages.footer_move, messages.footer_switch_group];
            match app.config_cursor.entry().item {
                ConfigItemId::OutputGuard | ConfigItemId::ResetAll | ConfigItemId::SaveAll => {
                    hints.push(messages.footer_select);
                }
                // A guarded tier is read-only, so neither adjusting nor editing applies.
                ConfigItemId::OutputTier if app.output_guard_active() => {}
                ConfigItemId::SearchCpuLimit
                | ConfigItemId::ReadBudget
                | ConfigItemId::GrepBudget
                | ConfigItemId::GlobBudget
                | ConfigItemId::RunBudget
                | ConfigItemId::JobOutputBudget => {
                    hints.push(messages.footer_adjust);
                    hints.push(app.config_messages().footer_edit);
                }
                _ => hints.push(messages.footer_adjust),
            }
            // The save hint only appears while something is unsaved, so its presence is itself
            // the answer to "did my edit register?".
            if app.config_is_dirty() {
                hints.push(messages.footer_save);
            }
            hints.push(messages.footer_back);
            hints
        }
        _ => vec![
            messages.footer_move,
            messages.footer_select,
            messages.footer_back,
        ],
    };
    frame.render_widget(
        Paragraph::new(Line::from(
            hints
                .into_iter()
                .enumerate()
                .flat_map(|(index, hint)| {
                    let mut spans = Vec::new();
                    if index > 0 {
                        spans.push(Span::styled("  ·  ", Style::default().fg(theme::border())));
                    }
                    spans.push(Span::styled(hint, Style::default().fg(theme::muted())));
                    spans
                })
                .collect::<Vec<_>>(),
        ))
        .alignment(Alignment::Center),
        area,
    );
}

fn jobs_footer(app: &App, maximum_width: usize) -> String {
    let job_messages = app.job_messages();
    let messages = app.messages();
    let full = [
        job_messages.footer_navigate,
        job_messages.footer_stop,
        job_messages.footer_refresh,
        messages.footer_back,
        job_messages.footer_horizontal,
        job_messages.footer_scroll,
        job_messages.footer_follow,
    ];
    if joined_footer_width(&full[..3]) > maximum_width {
        // The keys themselves are language-neutral and keep all essential controls discoverable
        // when localized labels cannot coexist on the minimum-width single row.
        return fit_footer_hints(
            &["↑↓", "Enter", "R", "Esc", "←→", "PgUp/PgDn", "F"],
            maximum_width,
        );
    }
    fit_footer_hints(&full, maximum_width)
}

fn fit_footer_hints(hints: &[&str], maximum_width: usize) -> String {
    const SEPARATOR: &str = "  ·  ";
    let mut selected = Vec::new();
    let mut width = 0;
    for hint in hints {
        let hint_width = Line::from(*hint).width();
        let added = hint_width
            + if selected.is_empty() {
                0
            } else {
                Line::from(SEPARATOR).width()
            };
        if width + added <= maximum_width {
            selected.push(*hint);
            width += added;
        }
    }
    selected.join(SEPARATOR)
}

fn joined_footer_width(hints: &[&str]) -> usize {
    const SEPARATOR: &str = "  ·  ";
    hints
        .iter()
        .map(|hint| Line::from(*hint).width())
        .sum::<usize>()
        + Line::from(SEPARATOR).width() * hints.len().saturating_sub(1)
}

fn render_toast(frame: &mut Frame<'_>, area: Rect, message: &str, color: Color) {
    let width = area.width.clamp(1, 58);
    let content_width = usize::from(width.saturating_sub(4).max(1));
    let wrapped_lines = message
        .lines()
        .map(|line| line.chars().count().max(1).div_ceil(content_width))
        .sum::<usize>();
    let height = u16::try_from(wrapped_lines.saturating_add(2))
        .unwrap_or(u16::MAX)
        .min(area.height.max(1));
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width),
        y: area.y,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(Style::default().fg(color).bg(theme::bg()))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(color)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .title_style(Style::default().fg(theme::muted()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .style(Style::default().fg(theme::fg()).bg(theme::bg()))
        .padding(Padding::horizontal(1))
}

fn selected_style() -> Style {
    Style::default()
        .fg(theme::accent())
        .bg(theme::bg_raised())
        .add_modifier(Modifier::BOLD)
}

fn inner(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(horizontal),
        y: area.y.saturating_add(vertical),
        width: area.width.saturating_sub(horizontal.saturating_mul(2)),
        height: area.height.saturating_sub(vertical.saturating_mul(2)),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn localized_check_name<'a>(app: &'a App, name: &'a str) -> &'a str {
    match name {
        "Codex profile" => "~/.codex",
        "Codex config" => app.messages().menu_config,
        "Applied state" => app.messages().menu_apply,
        "Provider output guard" => app.guard_messages().label,
        "Installed binary" => "FastCtx",
        "MCP server contract" => "FastCtx MCP",
        "Model tool surface" => "Codex ↔ FastCtx",
        "AGENTS guidance" => "AGENTS.md",
        "Search CPU limit" => app.config_messages().cpu_limit_label,
        "fastshell" => app.messages().fastshell_label,
        "fastshell MCP handshake" => "fastshell MCP",
        _ => name,
    }
}

fn config_narrow_summary(app: &App) -> String {
    let messages = app.messages();
    let entry = app.config_cursor.entry();
    let group = config::group_spec(entry.group);
    let value = config_value_label(
        app,
        entry.item,
        app.config_draft
            .value_with_guard(entry.item, app.output_guard_active()),
    );
    let item = config::item_label(
        entry.item,
        messages,
        app.config_messages(),
        app.guard_messages(),
        app.job_messages(),
        app.update_messages(),
    );
    // A pinned group carries no heading, so its trail starts at the item itself.
    let Some(group_title) = config::group_title(
        entry.group,
        messages,
        app.config_messages(),
        app.guard_messages(),
        app.update_messages(),
    ) else {
        return format!("{item} · {value}");
    };
    match entry.role {
        ConfigItemRole::Parent => format!("{group_title} › {item} · {value}"),
        ConfigItemRole::Child { .. } => format!(
            "{group_title} › {} › {item} · {value}",
            config::item_label(
                group.parent(),
                messages,
                app.config_messages(),
                app.guard_messages(),
                app.job_messages(),
                app.update_messages(),
            ),
        ),
    }
}

fn config_value_label(app: &App, item: ConfigItemId, value: ConfigValue) -> String {
    match value {
        ConfigValue::Tier(tier) => tier.display_name().to_string(),
        ConfigValue::GuardedTier(_) => "Guarded".to_string(),
        ConfigValue::Budget(budget) if budget.explicit => budget.level.label(),
        // A share that follows the tier says so instead of showing the number it happens to
        // resolve to: the two rendered identically, so nothing on the row distinguished the
        // budgets that move with the tier from the ones pinned against it.
        ConfigValue::Budget(_) => app.config_messages().automatic_label.to_string(),
        ConfigValue::Toggle(enabled) => toggle_label(app.messages(), enabled).to_string(),
        ConfigValue::Number(value) if item == ConfigItemId::JobStorageLimit => {
            if value >= 1_024 && value % 1_024 == 0 {
                format!("{} GiB", value / 1_024)
            } else {
                format!("{value} MiB")
            }
        }
        ConfigValue::Number(value) => value.to_string(),
        ConfigValue::ReplaceLimit(value) if value >= 1_024 && value % 1_024 == 0 => {
            format!("{} GiB", value / 1_024)
        }
        ConfigValue::ReplaceLimit(value) => format!("{value} MiB"),
        ConfigValue::CpuLimit(None) => app.config_messages().automatic_label.to_string(),
        ConfigValue::CpuLimit(Some(value)) => value.to_string(),
        ConfigValue::Source(source) => source.as_str().to_string(),
        ConfigValue::Action if item == ConfigItemId::SaveAll => {
            save_button_face(app, app.config_unsaved_count())
        }
        ConfigValue::Action => app.config_messages().reset_all_label.to_string(),
    }
}

fn config_value_color(value: ConfigValue) -> Color {
    match value {
        ConfigValue::Tier(tier) => tier_color(tier),
        ConfigValue::GuardedTier(_) => theme::warning(),
        // An explicit share is highlighted so the rows somebody has overridden stand apart from
        // the ones that will keep tracking the tier.
        ConfigValue::Budget(budget) if budget.explicit => theme::accent(),
        // Automatic is the resting state for every budget, so it recedes exactly like the CPU
        // limit's own automatic reading rather than competing with the shares somebody chose.
        ConfigValue::Budget(_) => theme::muted(),
        ConfigValue::Toggle(true) => theme::success(),
        ConfigValue::Toggle(false) => theme::muted(),
        ConfigValue::Number(_) => theme::fg(),
        ConfigValue::ReplaceLimit(value)
            if (MIN_REPLACE_FILE_LIMIT_MIB..=MAX_REPLACE_FILE_LIMIT_MIB).contains(&value) =>
        {
            theme::accent()
        }
        ConfigValue::ReplaceLimit(_) => theme::danger(),
        ConfigValue::CpuLimit(None) => theme::muted(),
        ConfigValue::CpuLimit(Some(value)) => {
            if search_parallelism::resolve(Some(value)).is_ok() {
                theme::accent()
            } else {
                theme::danger()
            }
        }
        ConfigValue::Source(_) => theme::accent(),
        ConfigValue::Action => theme::danger(),
    }
}

fn toggle_label(messages: &crate::control::i18n::Messages, enabled: bool) -> &'static str {
    if enabled {
        messages.enabled_label
    } else {
        messages.disabled_label
    }
}

/// Share plus the token ceiling it resolves to, for the detail pane and the editor preview.
///
/// The list row shows the bare share instead. A percentage alone would say nothing about how much
/// output it buys, but the detail pane always carries the ceiling for whichever row has focus, so
/// the column stays narrow without withholding the number the reader is deciding about.
fn budget_summary(level: ToolBudgetLevel, global: usize) -> String {
    format!("{} · {}", level.label(), level.ceiling(global))
}

#[cfg(test)]
mod tests {
    use super::render;

    use crate::control::doctor::{DoctorCheck, DoctorCheckStatus, DoctorReport};
    use crate::control::i18n::{ALL_LANGUAGES, Language};

    use crate::control::paths::ControlPaths;

    use crate::shell::jobs::{JobSourceSummary, JobSummary, JobSummaryStatus};
    use crate::tui::app::{App, Screen};
    use crate::tui::config::ConfigCursor;
    use crate::tui::jobs::{JobsDetail, JobsState};
    use crate::tui::theme::{self, ColorMode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::CellWidth;
    use ratatui::style::Color;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut text = String::new();
                let mut hidden_columns = 0usize;
                for x in 0..area.width {
                    if hidden_columns > 0 {
                        hidden_columns -= 1;
                        continue;
                    }
                    let cell = &buffer[(x, y)];
                    text.push_str(cell.symbol());
                    // A wide glyph owns the cells that trail it. Folding their reported width back
                    // in would skip the next real glyph too, which silently ate every second
                    // character out of any CJK assertion.
                    hidden_columns = usize::from(cell.cell_width()).saturating_sub(1);
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn contains_visible_text(buffer: &str, expected: &str) -> bool {
        let buffer = buffer
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        let expected = expected
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        buffer.contains(&expected)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn test_backend_renders_first_run_apply_preview_and_unapply_choices() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.language = Language::En;
        app.selected = 0;
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains(app.messages().language_prompt));

        app.settings.language = Some("en".to_string());
        app.screen = Screen::ApplyHome;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains(app.messages().preview_title));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.selected, 0);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let receipt = buffer_text(&terminal);
        assert!(receipt.contains(app.messages().receipt_title));
        assert!(receipt.contains(app.messages().restart_notice));

        app.screen = Screen::ApplyHome;
        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert!(buffer_text(&terminal).contains(app.messages().preview_title));
    }

    #[test]
    fn connections_render_both_hosts_and_dsh_scope_at_standard_and_narrow_widths() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.screen = Screen::Connections;
        app.selected = 1;

        for (width, height) in [(100, 30), (42, 10)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(contains_visible_text(&text, "DeepSeek Harness"), "{text}");
            if width >= 100 {
                assert!(contains_visible_text(&text, "ChatGPT / Codex"), "{text}");
                assert!(
                    contains_visible_text(&text, "Host-wide (all DSH profiles)"),
                    "{text}"
                );
                assert!(contains_visible_text(&text, "300000 ms"), "{text}");
                assert!(contains_visible_text(&text, "Patch"), "{text}");
                assert!(contains_visible_text(&text, "rdis.patch.yml"), "{text}");
            }
        }
    }

    #[test]
    fn all_languages_render_at_cjk_and_narrow_boundaries_without_panicking() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        for language in ALL_LANGUAGES {
            let mut app = App::for_test(paths.clone(), executable.clone());
            app.language = Language::parse(language.code()).unwrap();
            app.screen = Screen::Main;
            for (width, height) in [(100, 30), (52, 12), (40, 10), (39, 8)] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                let text = buffer_text(&terminal);
                assert!(!text.trim().is_empty());
                if width == 100 {
                    assert!(
                        contains_visible_text(&text, app.messages().main_title),
                        "{}",
                        language.code()
                    );
                } else if width == 40 {
                    let selected_label = [
                        app.messages().menu_apply,
                        app.messages().menu_config,
                        app.messages().menu_status,
                        app.messages().menu_about,
                        app.messages().menu_language,
                    ][app.selected];
                    assert!(
                        contains_visible_text(&text, selected_label),
                        "{} selected item\n{text}",
                        language.code(),
                    );
                } else if width == 39 {
                    assert!(text.contains("40×9"), "{}\n{text}", language.code());
                }
            }
        }
    }

    #[test]
    fn jobs_loading_empty_permission_error_and_ready_states_render_in_all_languages() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        for language in ALL_LANGUAGES {
            let mut app = App::for_test(paths.clone(), executable.clone());
            app.settings.language = Some(language.code().to_string());
            app.language = language;
            app.screen = Screen::Jobs;
            let render_once = |app: &mut App| {
                let backend = TestBackend::new(100, 24);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, app)).unwrap();
                buffer_text(&terminal)
            };

            app.jobs_state = JobsState::Loading;
            let loading = render_once(&mut app);
            assert!(
                contains_visible_text(&loading, app.job_messages().loading),
                "{} loading\n{loading}",
                language.code()
            );

            app.jobs_state = JobsState::Empty;
            let empty = render_once(&mut app);
            let empty_note_prefix = app
                .job_messages()
                .empty_note
                .chars()
                .take(32)
                .collect::<String>();
            for expected in [app.job_messages().empty, empty_note_prefix.as_str()] {
                assert!(
                    contains_visible_text(&empty, expected),
                    "{} missing {expected}\n{empty}",
                    language.code()
                );
            }

            app.jobs_state = JobsState::PermissionDenied("access denied".to_string());
            let permission = render_once(&mut app);
            for expected in ["access denied", app.job_messages().permission_title] {
                assert!(
                    contains_visible_text(&permission, expected),
                    "{} missing {expected}\n{permission}",
                    language.code()
                );
            }

            app.jobs_state = JobsState::Error("job storage unavailable".to_string());
            let error = render_once(&mut app);
            let error_note_prefix = app
                .job_messages()
                .error_note
                .chars()
                .take(32)
                .collect::<String>();
            for expected in [
                "job storage unavailable",
                app.job_messages().error_title,
                error_note_prefix.as_str(),
            ] {
                assert!(
                    contains_visible_text(&error, expected),
                    "{} missing {expected}\n{error}",
                    language.code()
                );
            }

            let summary = JobSummary {
                id: "j-000001".to_string(),
                command: "printf tail".to_string(),
                cwd: "/workspace".to_string(),
                started_at: "2026-07-16T10:00:00Z".to_string(),
                status: JobSummaryStatus::Running,
                source: JobSourceSummary {
                    key: "source-1".to_string(),
                    tag: "a001".to_string(),
                    server_pid: 7,
                    parent_executable: Some("codex".to_string()),
                    server_cwd: "/workspace".to_string(),
                },
            };
            app.jobs_state = JobsState::ready(vec![summary]);
            app.jobs_selected = 0;
            app.jobs_detail = JobsDetail::default();
            let ready_loading = render_once(&mut app);
            assert!(contains_visible_text(
                &ready_loading,
                app.job_messages().loading
            ));

            app.jobs_detail.job_id = Some("j-000001".to_string());
            app.jobs_detail.tail.lines = vec!["older line".to_string(), "newest line".to_string()];
            let ready = render_once(&mut app);
            for expected in [
                "j-000001",
                "/workspace",
                "#a001",
                app.job_messages().started_label,
                app.job_messages().elapsed_label,
                "2026-07-16 10:00:00 UTC",
                "newest line",
                app.job_messages().footer_scope,
                app.job_messages().footer_refresh,
            ] {
                assert!(
                    contains_visible_text(&ready, expected),
                    "{} missing {expected}\n{ready}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn guarded_tier_and_disable_warning_render_in_every_language_and_supported_width() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let local = crate::control::provider::detect_bytes(Some(
            b"model_provider='custom'\n[model_providers.custom]\nname='Third Party'\n",
        ));

        for language in ALL_LANGUAGES {
            let mut app = App::for_test(paths.clone(), executable.clone());
            app.settings.language = Some(language.code().to_string());
            app.language = language;
            app.provider_detection = local.clone();
            app.screen = Screen::Config;
            app.config_cursor = ConfigCursor::default();
            let backend = TestBackend::new(100, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            for expected in ["Guarded", "10000", "9000"] {
                assert!(
                    contains_visible_text(&text, expected),
                    "{} missing {expected}\n{text}",
                    language.code()
                );
            }

            app.screen = Screen::ConfigOutputGuardConfirm;
            app.selected = 0;
            for (width, height) in [(100, 30), (52, 18), (40, 10)] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                let text = buffer_text(&terminal);
                for expected in ["Guarded", "✕", "✓"] {
                    assert!(
                        contains_visible_text(&text, expected),
                        "{} missing {expected} at {width}x{height}\n{text}",
                        language.code()
                    );
                }
                assert_eq!(app.selected, 0);
            }
        }
    }

    #[test]
    fn monochrome_mode_keeps_status_semantics_and_uses_no_colors() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Status;
        app.status = crate::tui::app::StatusState::Ready(DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "Installed binary",
                    status: DoctorCheckStatus::Pass,
                    detail: "ready".to_string(),
                    remedy: None,
                },
                DoctorCheck {
                    name: "Codex profile",
                    status: DoctorCheckStatus::Info,
                    detail: "not applied".to_string(),
                    remedy: None,
                },
                DoctorCheck {
                    name: "Applied state",
                    status: DoctorCheckStatus::Fail,
                    detail: "repair required".to_string(),
                    remedy: Some("re-apply".to_string()),
                },
            ],
        });
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        theme::with_test_mode(ColorMode::Monochrome, || {
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        });

        let text = buffer_text(&terminal);
        for marker in ["✓", "○", "×"] {
            assert!(text.contains(marker), "missing {marker}\n{text}");
        }
        for cell in terminal.backend().buffer().content() {
            assert_eq!(cell.fg, Color::Reset);
            assert_eq!(cell.bg, Color::Reset);
        }
    }
}
