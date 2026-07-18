//! Adaptive ratatui views and the shared TUI theme.

use super::app::{App, Screen, StatusState};
use super::config::{
    self, ConfigGroupId, ConfigItemId, ConfigItemRole, ConfigListRow, ConfigValue,
};
use super::jobs::{JobGroup, JobsState, display_output_line, grouped_jobs, source_count};
use super::theme;
use crate::control::apply::{PreviewAction, PreviewItem, PreviewTarget};
use crate::control::doctor::DoctorCheckStatus;
use crate::control::i18n::ALL_LANGUAGES;
use crate::control::settings::{Tier, ToolBudgetLevel};
use crate::shell::jobs::{JobSourceSummary, JobSummary};
use crate::update::StartupUpdate;
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
        Screen::Config | Screen::Jobs => false,
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
        Screen::UpdateAvailable | Screen::UpdatePending => render_update(frame, app, area),
        Screen::UpdateChecking => render_loading(frame, app, area, app.update_messages().checking),
        Screen::Language { .. } => render_languages(frame, app, area),
        Screen::Main => render_main(frame, app, area),
        Screen::ApplyHome => render_apply_home(frame, app, area),
        Screen::ApplyLoading | Screen::UnapplyLoading => {
            render_loading(frame, app, area, app.messages().loading)
        }
        Screen::ApplyPreview => render_preview(frame, app, area, true),
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
        Screen::UnapplyPreview => render_preview(frame, app, area, false),
        Screen::UnapplyConfirm => render_confirmation(
            frame,
            app,
            area,
            app.messages().confirm_unapply,
            theme::danger(),
        ),
        Screen::Config => render_config(frame, app, area),
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

fn render_update(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let messages = app.update_messages();
    let (title, body, primary) = match &app.update_state {
        StartupUpdate::Available(plan) => (
            messages.available_title,
            messages
                .available_body
                .replace("{current}", env!("CARGO_PKG_VERSION"))
                .replace("{latest}", plan.target_version())
                .replace("{source}", &plan.source_label()),
            messages.action_update,
        ),
        StartupUpdate::NpmPending {
            release_version,
            registry_version,
        } => (
            messages.pending_title,
            messages
                .pending_body
                .replace("{latest}", release_version)
                .replace("{registry}", registry_version),
            app.messages().action_retry,
        ),
        _ => (
            messages.check_failed,
            app.messages().operation_failed.to_string(),
            app.messages().action_retry,
        ),
    };
    let popup = centered_rect(78, 58, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(inner(popup, 2, 1));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme::fg()))
            .block(panel(title).border_style(Style::default().fg(theme::accent())))
            .wrap(Wrap { trim: false }),
        popup,
    );
    render_labeled_actions(
        frame,
        chunks[1],
        app.selected,
        primary,
        messages.action_continue,
    );
}

fn render_labeled_actions(
    frame: &mut Frame<'_>,
    area: Rect,
    selected: usize,
    primary: &str,
    secondary: &str,
) {
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {primary}  "),
                style(selected == 0, theme::accent()),
            ),
            Span::raw("     "),
            Span::styled(
                format!("  {secondary}  "),
                style(selected == 1, theme::muted()),
            ),
        ]))
        .alignment(Alignment::Center),
        area,
    );
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
        messages.menu_status,
        messages.menu_about,
        messages.menu_language,
    ];
    let items = labels
        .iter()
        .map(|label| ListItem::new(format!(" {label}")))
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
    let mut details = vec![
        Line::styled(
            labels[app.selected],
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        detail_line("FastCtx", &format!("v{}", env!("CARGO_PKG_VERSION"))),
        detail_line(messages.tier_label, app.settings.tier.display_name()),
        detail_line(messages.menu_language, app.language.native_name()),
    ];
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
    match &app.update_state {
        StartupUpdate::Available(plan) => details.push(detail_line(
            app.update_messages().action_check,
            &format!("v{} · U", plan.target_version()),
        )),
        StartupUpdate::NpmPending {
            release_version, ..
        } => details.push(detail_line(
            app.update_messages().action_check,
            &format!("v{release_version} · U"),
        )),
        _ => {}
    }
    frame.render_widget(
        Paragraph::new(details)
            .block(panel(messages.app_title))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

fn detail_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}  "), Style::default().fg(theme::muted())),
        Span::styled(value.to_string(), Style::default().fg(theme::fg())),
    ])
}

fn tier_note(app: &App, tier: Tier) -> &'static str {
    match tier {
        Tier::Standard => app.messages().tier_note_standard,
        Tier::High => app.messages().tier_note_high,
        Tier::ExtraHigh => app.messages().tier_note_extra_high,
    }
}

/// Tier colors use neutral white for Standard, amber caution for High, and vermilion warning for Extra High.
/// The one-way neutral-to-warning progression signals increasing caution; green is deliberately avoided so upgrades do not read as better.
fn tier_color(tier: Tier) -> Color {
    match tier {
        Tier::Standard => theme::accent(),
        Tier::High => theme::warning(),
        Tier::ExtraHigh => theme::danger(),
    }
}

fn render_apply_home(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let messages = app.messages();
    let applied = app.settings.applied.is_some();
    let items = vec![
        ListItem::new(format!(" {}", messages.action_apply)),
        ListItem::new(format!(" {}", messages.action_unapply)),
    ];
    let mut state = ListState::default().with_selected(Some(app.selected));
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
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                if applied {
                    "✓ FastCtx"
                } else {
                    "○ FastCtx"
                },
                Style::default()
                    .fg(if applied {
                        theme::success()
                    } else {
                        theme::warning()
                    })
                    .add_modifier(Modifier::BOLD),
            ),
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
            detail_line("read", budget_label(app.settings.tool_budgets.read)),
            detail_line("grep", budget_label(app.settings.tool_budgets.grep)),
            detail_line("glob", budget_label(app.settings.tool_budgets.glob)),
            detail_line("run", budget_label(app.settings.tool_budgets.run)),
            detail_line(
                "job_output",
                budget_label(app.settings.tool_budgets.job_output),
            ),
        ])
        .block(panel(messages.config_title)),
        chunks[1],
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
    let detail_height = if compact {
        2
    } else {
        content_area.height.saturating_sub(6).clamp(4, 9)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(detail_height)])
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
            ConfigListRow::Group(group) => table_rows.push(Row::new(vec![
                Cell::from(Line::styled(
                    config::group_title(group, messages).to_string(),
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
    let mut table = Table::new(
        table_rows,
        [Constraint::Percentage(42), Constraint::Percentage(58)],
    );
    table = table.column_spacing(if compact { 1 } else { 2 });
    if !compact {
        table = table.block(panel(messages.menu_config));
    }
    frame.render_widget(table, chunks[0]);

    let entry = app.config_cursor.entry();
    let detail = match app.config_draft.value(entry.item) {
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
        ConfigValue::Budget(level) => {
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    config::item_label(
                        config::group_spec(entry.group).parent(),
                        messages,
                        app.job_messages(),
                    ),
                    Style::default().fg(theme::muted()),
                ),
                Span::styled("  ›  ", Style::default().fg(theme::border())),
                Span::styled(
                    config::item_label(entry.item, messages, app.job_messages()),
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", budget_label(level)),
                    Style::default()
                        .fg(theme::fg())
                        .add_modifier(Modifier::BOLD),
                ),
            ])];
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
                _ => messages.extensions_note,
            };
            vec![
                Line::from(vec![
                    Span::styled(
                        config::item_label(entry.item, messages, app.job_messages()),
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
                Line::raw(""),
                Line::styled(
                    messages.extensions_note,
                    Style::default().fg(theme::muted()),
                ),
            ]
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
                        config::item_label(entry.item, messages, app.job_messages()),
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ·  ", Style::default().fg(theme::border())),
                    Span::styled(
                        config_value_label(messages, entry.item, ConfigValue::Number(value)),
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
    };
    let mut detail = Paragraph::new(detail).wrap(Wrap { trim: false });
    if !compact {
        detail = detail.block(panel(config::group_title(entry.group, messages)));
    }
    frame.render_widget(detail, chunks[1]);
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
    let value = app.config_draft.value(item);
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
                config::item_label(item, app.messages(), app.job_messages()).to_string(),
                if selected {
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::muted())
                },
            ),
        ])),
        Cell::from(Line::from(vec![
            Span::styled(
                "‹ ",
                Style::default().fg(if selected {
                    theme::accent()
                } else {
                    theme::border()
                }),
            ),
            Span::styled(
                config_value_label(app.messages(), item, value),
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
        ])),
    ])
    .style(base)
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
        Line::styled("MIT OR Apache-2.0", Style::default().fg(theme::muted())),
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
    let popup = centered_rect(66, 38, area);
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
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                prompt,
                Style::default()
                    .fg(theme::fg())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::from(vec![
                Span::styled("  ✕  ", no_style),
                Span::raw("     "),
                Span::styled("  ✓  ", yes_style),
            ])
            .alignment(Alignment::Center),
        ])
        .alignment(Alignment::Center)
        .block(panel(prompt).border_style(Style::default().fg(color)))
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
            Line::styled("MIT OR Apache-2.0", Style::default().fg(theme::muted())),
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
        Screen::UpdateAvailable => app.update_messages().available_title.to_string(),
        Screen::UpdatePending => app.update_messages().pending_title.to_string(),
        Screen::UpdateChecking => app.update_messages().checking.to_string(),
        Screen::Language { .. } => format!(
            "{} · {}",
            ALL_LANGUAGES[app.selected].code(),
            ALL_LANGUAGES[app.selected].native_name()
        ),
        Screen::Main => [
            messages.menu_apply,
            messages.menu_config,
            app.job_messages().menu,
            messages.menu_status,
            messages.menu_about,
            messages.menu_language,
        ][app.selected]
            .to_string(),
        Screen::ApplyHome => {
            [messages.action_apply, messages.action_unapply][app.selected].to_string()
        }
        Screen::Config => config_narrow_summary(app),
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
            .fg(theme::fg())
            .add_modifier(Modifier::BOLD),
    ));
    if matches!(app.screen, Screen::UpdateAvailable | Screen::UpdatePending) {
        let detail = match &app.update_state {
            StartupUpdate::Available(plan) => format!(
                "v{} → v{} · {}",
                env!("CARGO_PKG_VERSION"),
                plan.target_version(),
                plan.source_label()
            ),
            StartupUpdate::NpmPending {
                release_version,
                registry_version,
            } => format!("GitHub v{release_version} · npm v{registry_version}"),
            _ => String::new(),
        };
        lines.push(Line::styled(
            truncate_end(&detail, usize::from(area.width.saturating_sub(4))),
            Style::default().fg(theme::muted()),
        ));
        let primary = if app.screen == Screen::UpdateAvailable {
            app.update_messages().action_update
        } else {
            messages.action_retry
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
        Screen::UpdateAvailable | Screen::UpdatePending => {
            vec![messages.footer_move, messages.footer_select]
        }
        Screen::UpdateChecking => vec![app.update_messages().checking],
        Screen::Main
            if matches!(
                &app.update_state,
                StartupUpdate::Available(_) | StartupUpdate::NpmPending { .. }
            ) =>
        {
            vec![
                messages.footer_move,
                messages.footer_select,
                app.update_messages().action_check,
                messages.footer_quit,
            ]
        }
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
        Screen::Config => vec![
            messages.footer_move,
            messages.footer_switch_group,
            messages.footer_adjust,
            messages.footer_save,
            messages.footer_cancel,
        ],
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
        "Installed binary" => "FastCtx",
        "MCP handshake" => "MCP",
        "AGENTS guidance" => "AGENTS.md",
        "fastshell" => app.messages().fastshell_label,
        "fastshell MCP handshake" => "fastshell MCP",
        _ => name,
    }
}

fn config_narrow_summary(app: &App) -> String {
    let messages = app.messages();
    let entry = app.config_cursor.entry();
    let group = config::group_spec(entry.group);
    let value = config_value_label(messages, entry.item, app.config_draft.value(entry.item));
    match entry.role {
        ConfigItemRole::Parent => format!(
            "{} › {} · {}",
            config::group_title(entry.group, messages),
            config::item_label(entry.item, messages, app.job_messages()),
            value
        ),
        ConfigItemRole::Child { .. } => format!(
            "{} › {} › {} · {}",
            config::group_title(entry.group, messages),
            config::item_label(group.parent(), messages, app.job_messages()),
            config::item_label(entry.item, messages, app.job_messages()),
            value
        ),
    }
}

fn config_value_label(
    messages: &crate::control::i18n::Messages,
    item: ConfigItemId,
    value: ConfigValue,
) -> String {
    match value {
        ConfigValue::Tier(tier) => tier.display_name().to_string(),
        ConfigValue::Budget(level) => budget_label(level).to_string(),
        ConfigValue::Toggle(enabled) => toggle_label(messages, enabled).to_string(),
        ConfigValue::Number(value) if item == ConfigItemId::JobStorageLimit => {
            if value >= 1_024 && value % 1_024 == 0 {
                format!("{} GiB", value / 1_024)
            } else {
                format!("{value} MiB")
            }
        }
        ConfigValue::Number(value) => value.to_string(),
    }
}

fn config_value_color(value: ConfigValue) -> Color {
    match value {
        ConfigValue::Tier(tier) => tier_color(tier),
        ConfigValue::Budget(_) => theme::fg(),
        ConfigValue::Toggle(true) => theme::success(),
        ConfigValue::Toggle(false) => theme::muted(),
        ConfigValue::Number(_) => theme::fg(),
    }
}

fn toggle_label(messages: &crate::control::i18n::Messages, enabled: bool) -> &'static str {
    if enabled {
        messages.enabled_label
    } else {
        messages.disabled_label
    }
}

fn budget_label(level: ToolBudgetLevel) -> &'static str {
    match level {
        ToolBudgetLevel::Inherit => "100%",
        ToolBudgetLevel::Percent75 => "75%",
        ToolBudgetLevel::Percent50 => "50%",
        ToolBudgetLevel::Percent25 => "25%",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        elapsed_hms_at, exact_started_at, job_list_columns, jobs_footer, relative_started_at_at,
        render, truncate_display_width, wrap_detail_lines,
    };
    use crate::control::apply::OperationReceipt;
    use crate::control::doctor::{DoctorCheck, DoctorCheckStatus, DoctorReport};
    use crate::control::i18n::{ALL_LANGUAGES, Language};
    use crate::control::paths::ControlPaths;
    use crate::shell::jobs::{JobSourceSummary, JobSummary, JobSummaryStatus};
    use crate::tui::app::{App, Screen};
    use crate::tui::config::{ConfigCursor, ConfigItemId};
    use crate::tui::jobs::{JobsDetail, JobsState};
    use crate::tui::theme::{self, ColorMode, Theme};
    use crate::update::{StartupUpdate, UpdatePlan};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::CellWidth;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut text = String::new();
                let mut hidden_columns = 0;
                for x in 0..area.width {
                    let cell = &buffer[(x, y)];
                    if hidden_columns == 0 {
                        text.push_str(cell.symbol());
                    }
                    hidden_columns = hidden_columns.max(cell.cell_width()).saturating_sub(1);
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

    #[test]
    fn buffer_text_ignores_backend_cells_hidden_by_wide_symbols() {
        let backend = TestBackend::new(4, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new("abcd"), frame.area()))
            .unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new("界cd"), frame.area()))
            .unwrap();

        assert_eq!(buffer_text(&terminal), "界cd");
    }

    #[test]
    fn narrow_detail_wrapping_preserves_every_grapheme_and_style() {
        let danger = Style::default()
            .fg(theme::danger())
            .add_modifier(Modifier::CROSSED_OUT);
        let muted = Style::default().fg(theme::muted());
        let wrapped = wrap_detail_lines(
            vec![Line::from(vec![
                Span::styled("界界", danger),
                Span::styled(" abcdef", muted),
            ])],
            4,
        );

        assert!(wrapped.iter().all(|line| line.width() <= 4));
        let flattened = wrapped
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(flattened, "界界 abcdef");
        assert!(
            wrapped
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains('界') && span.style == danger)
        );
        assert!(
            wrapped
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.contains('a') && span.style == muted)
        );
    }

    #[test]
    fn update_available_has_a_dedicated_adaptive_screen() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.screen = Screen::UpdateAvailable;
        app.update_state = StartupUpdate::Available(UpdatePlan::GithubRelease {
            target_version: "0.2.0".to_string(),
            archive_name: "fixture.zip".to_string(),
            archive_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/fixture.zip"
                .to_string(),
            checksums_url: "https://github.com/yc-duan/fastctx/releases/download/v0.2.0/SHA256SUMS"
                .to_string(),
        });

        for (width, height) in [(100, 24), (40, 10)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            for expected in [
                app.update_messages().available_title,
                app.update_messages().action_update,
                app.update_messages().action_continue,
                "v0.1.0 → v0.2.0",
                "GitHub Release",
            ] {
                assert!(
                    contains_visible_text(&text, expected),
                    "{width}x{height} missing {expected}\n{text}"
                );
            }
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn symbol_color(terminal: &Terminal<TestBackend>, symbol: &str) -> Color {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buffer[(x, y)];
                if cell.symbol() == symbol {
                    return cell.fg;
                }
            }
        }
        panic!("symbol {symbol:?} was not rendered");
    }

    #[test]
    fn job_times_are_precise_live_and_honest_for_invalid_or_future_values() {
        let now = OffsetDateTime::parse("2026-07-17T12:34:56Z", &Rfc3339).unwrap();
        assert_eq!(
            exact_started_at("2026-07-17T14:34:50+02:00").as_deref(),
            Some("2026-07-17 12:34:50 UTC")
        );
        assert_eq!(
            elapsed_hms_at("2026-07-15T12:34:56Z", now).as_deref(),
            Some("48:00:00")
        );
        assert_eq!(
            relative_started_at_at("2026-07-17T12:33:56Z", now).as_deref(),
            Some("1m")
        );
        assert_eq!(
            elapsed_hms_at("2026-07-18T12:34:56Z", now).as_deref(),
            Some("00:00:00")
        );
        assert!(exact_started_at("not-a-time").is_none());
        assert!(elapsed_hms_at("not-a-time", now).is_none());
        assert!(relative_started_at_at("not-a-time", now).is_none());
    }

    #[test]
    fn job_list_columns_align_ids_and_put_ascii_or_cjk_ellipsis_at_one_edge() {
        let source = JobSourceSummary {
            key: "source".to_string(),
            tag: "abcdef".to_string(),
            server_pid: 7,
            parent_executable: Some("codex".to_string()),
            server_cwd: "/workspace".to_string(),
        };
        let make_job = |id: &str, command: &str| JobSummary {
            id: id.to_string(),
            command: command.to_string(),
            cwd: "/workspace".to_string(),
            started_at: "2026-07-17T12:33:56Z".to_string(),
            status: JobSummaryStatus::Running,
            source: source.clone(),
        };
        let now = OffsetDateTime::parse("2026-07-17T12:34:56Z", &Rfc3339).unwrap();
        let available = 31;
        let ascii = job_list_columns(
            &make_job("j-000001", "abcdefghijklmnopqrstuvwxyz"),
            available,
            now,
        );
        let cjk = job_list_columns(
            &make_job("j-000002", "构建任务输出非常非常长"),
            available,
            now,
        );

        for (age, id, command) in [&ascii, &cjk] {
            assert_eq!(Line::from(age.as_str()).width(), 4);
            assert_eq!(Line::from(id.as_str()).width(), 8);
            assert_eq!(Line::from(command.as_str()).width(), available - 19);
            assert!(command.ends_with('…'));
        }
        assert_eq!(ascii.1, "j-000001");
        assert_eq!(cjk.1, "j-000002");
        assert_eq!(Line::from(truncate_display_width("界界界界", 7)).width(), 7);
    }

    fn row_containing(terminal: &Terminal<TestBackend>, expected: &str) -> Option<u16> {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        (0..area.height).find(|y| {
            let row = (0..area.width)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>();
            contains_visible_text(&row, expected)
        })
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
    fn main_menu_omits_the_tier_explainer_but_keeps_the_selected_tier() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Main;
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(contains_visible_text(
            &text,
            app.settings.tier.display_name()
        ));
        assert!(!contains_visible_text(
            &text,
            app.messages().tier_note_standard
        ));
    }

    #[test]
    fn every_budget_detail_includes_its_localized_tool_summary() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Config;
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let expected = [
            (ConfigItemId::ReadBudget, app.messages().read_tool_note),
            (ConfigItemId::GrepBudget, app.messages().grep_tool_note),
            (ConfigItemId::GlobBudget, app.messages().glob_tool_note),
            (ConfigItemId::RunBudget, app.messages().run_tool_note),
            (
                ConfigItemId::JobOutputBudget,
                app.messages().job_output_tool_note,
            ),
        ];

        app.config_cursor = ConfigCursor::default().next();
        for (item, note) in expected {
            assert_eq!(app.config_cursor.entry().item, item);
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            assert!(contains_visible_text(&text, note), "missing {note}\n{text}");
            app.config_cursor = app.config_cursor.next();
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
    fn narrow_detail_screens_keep_their_operational_state_visible() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        std::fs::write(&app.paths.codex_config, b"tool_output_token_limit = 7000\n").unwrap();
        let backend = TestBackend::new(40, 9);
        let mut terminal = Terminal::new(backend).unwrap();
        let render_text = |terminal: &mut Terminal<TestBackend>, app: &mut App| {
            terminal.draw(|frame| render(frame, app)).unwrap();
            buffer_text(terminal)
        };

        app.screen = Screen::ApplyHome;
        app.selected = 0;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::ApplyPreview);
        let mut apply_preview_pages = String::new();
        for _ in 0..128 {
            let page = render_text(&mut terminal, &mut app);
            apply_preview_pages.push_str(&page);
            apply_preview_pages.push('\n');
            if !app.detail_viewport.can_move_down() {
                break;
            }
            app.handle_key(key(KeyCode::Down));
        }
        assert!(!app.detail_viewport.can_move_down());
        for expected in [
            app.messages().verb_install,
            app.messages().purpose_binary,
            app.messages().purpose_codex_config,
            app.messages().purpose_agents,
            app.messages().purpose_receipt,
            "tool_output_token_limit",
            app.messages().conflict_warning,
        ] {
            let prefix = expected.chars().take(28).collect::<String>();
            assert!(
                contains_visible_text(&apply_preview_pages, &prefix),
                "missing {expected}\n{apply_preview_pages}"
            );
        }

        app.apply_plan = None;
        app.screen = Screen::ApplyHome;
        app.selected = 1;
        app.handle_key(key(KeyCode::Enter));
        app.execute_pending();
        assert_eq!(app.screen, Screen::UnapplyPreview);
        let unapply_preview = render_text(&mut terminal, &mut app);
        assert!(
            contains_visible_text(&unapply_preview, "Stop 0 running"),
            "{unapply_preview}"
        );

        app.screen = Screen::Status;
        app.status = crate::tui::app::StatusState::Loading;
        let loading = render_text(&mut terminal, &mut app);
        assert!(contains_visible_text(&loading, app.messages().loading));

        app.status = crate::tui::app::StatusState::Empty;
        let empty = render_text(&mut terminal, &mut app);
        assert!(contains_visible_text(&empty, app.messages().empty));

        app.status = crate::tui::app::StatusState::Error("status fixture failed".to_string());
        let error = render_text(&mut terminal, &mut app);
        for expected in ["status fixture failed", app.messages().action_retry] {
            assert!(contains_visible_text(&error, expected), "{error}");
        }

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
        let ready = render_text(&mut terminal, &mut app);
        for expected in ["✓ 1", "○ 1", "× 1", "repair required"] {
            assert!(contains_visible_text(&ready, expected), "{ready}");
        }
        app.handle_key(key(KeyCode::End));
        let ready_end = render_text(&mut terminal, &mut app);
        assert!(contains_visible_text(&ready_end, "ready"), "{ready_end}");

        app.screen = Screen::Receipt;
        app.receipt = Some(OperationReceipt {
            changed_targets: 3,
            notes: vec!["receipt detail".to_string()],
        });
        let receipt = render_text(&mut terminal, &mut app);
        for expected in ["✓ 3", "receipt detail"] {
            assert!(contains_visible_text(&receipt, expected), "{receipt}");
        }
        let restart_prefix = app
            .messages()
            .restart_notice
            .chars()
            .take(24)
            .collect::<String>();
        assert!(
            contains_visible_text(&receipt, &restart_prefix),
            "{receipt}"
        );

        app.screen = Screen::OperationFailed;
        app.error = Some("operation fixture failed".to_string());
        let failure = render_text(&mut terminal, &mut app);
        for expected in ["operation fixture failed", app.messages().action_retry] {
            assert!(contains_visible_text(&failure, expected), "{failure}");
        }

        app.screen = Screen::About;
        let about = render_text(&mut terminal, &mut app);
        assert!(
            contains_visible_text(&about, "https://github.com/yc-duan/fastctx"),
            "{about}"
        );
        app.handle_key(key(KeyCode::End));
        let about_end = render_text(&mut terminal, &mut app);
        assert!(
            contains_visible_text(&about_end, "MIT OR Apache-2.0"),
            "{about_end}"
        );

        for (width, height) in [(51, 11), (52, 12)] {
            app.screen = Screen::Main;
            let backend = TestBackend::new(width, height);
            let mut boundary_terminal = Terminal::new(backend).unwrap();
            let _ = render_text(&mut boundary_terminal, &mut app);
            app.screen = Screen::Status;
            let boundary = render_text(&mut boundary_terminal, &mut app);
            for expected in ["✓ 1", "○ 1", "× 1"] {
                assert!(
                    contains_visible_text(&boundary, expected),
                    "{width}x{height} missing {expected}\n{boundary}"
                );
            }
        }
    }

    #[test]
    fn every_language_can_scroll_to_narrow_preview_purpose_and_shared_limit_warning() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        std::fs::write(&paths.codex_config, b"tool_output_token_limit = 7000\n").unwrap();
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();

        for language in ALL_LANGUAGES {
            let mut app = App::for_test(paths.clone(), executable.clone());
            app.settings.language = Some(language.code().to_string());
            app.language = language;
            app.screen = Screen::ApplyHome;
            app.selected = 0;
            app.handle_key(key(KeyCode::Enter));
            app.execute_pending();
            assert_eq!(app.screen, Screen::ApplyPreview);
            let mut pages = String::new();

            for _ in 0..128 {
                let backend = TestBackend::new(40, 9);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                pages.push_str(&buffer_text(&terminal));
                pages.push('\n');
                if !app.detail_viewport.can_move_down() {
                    break;
                }
                app.handle_key(key(KeyCode::Down));
            }

            for expected in [
                app.messages().purpose_binary,
                app.messages().conflict_warning,
            ] {
                assert!(
                    contains_visible_text(&pages, expected),
                    "{} missing {expected}\n{pages}",
                    language.code()
                );
            }
            assert!(
                contains_visible_text(&pages, "tool_output_token_limit"),
                "{} missing technical detail\n{pages}",
                language.code()
            );
            assert!(
                pages.contains('↓'),
                "{} missing the scroll affordance\n{pages}",
                language.code()
            );
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

            app.jobs_state = JobsState::Error("spool unavailable".to_string());
            let error = render_once(&mut app);
            let error_note_prefix = app
                .job_messages()
                .error_note
                .chars()
                .take(32)
                .collect::<String>();
            for expected in [
                "spool unavailable",
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
    fn jobs_dashboard_is_adaptive_and_right_arrow_reveals_wide_output() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Jobs;
        let summary = |id: &str, source: &str, tag: &str, pid: u32| JobSummary {
            id: id.to_string(),
            command: format!("printf {id}"),
            cwd: format!("/{source}/work"),
            started_at: "2026-07-16T10:00:00Z".to_string(),
            status: JobSummaryStatus::Running,
            source: JobSourceSummary {
                key: source.to_string(),
                tag: tag.to_string(),
                server_pid: pid,
                parent_executable: Some("codex".to_string()),
                server_cwd: format!("/{source}"),
            },
        };
        app.jobs_state = JobsState::ready(vec![
            summary("j-000001", "workspace-a", "a001", 7),
            summary("j-000002", "workspace-b", "b002", 8),
        ]);
        app.jobs_detail.job_id = Some("j-000001".to_string());
        app.jobs_detail.tail.lines = vec!["PREFIX00VISIBLE-TO-THE-RIGHT".to_string()];
        app.jobs_detail.horizontal_offset = 8;

        for (width, height) in [(100, 24), (70, 18), (40, 10)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let text = buffer_text(&terminal);
            let footer_rows = text
                .lines()
                .filter(|line| contains_visible_text(line, app.job_messages().footer_navigate))
                .collect::<Vec<_>>();
            assert_eq!(
                footer_rows.len(),
                1,
                "{width}x{height} must render one Jobs footer row\n{text}"
            );
            for expected in [
                app.job_messages().footer_stop,
                app.job_messages().footer_refresh,
            ] {
                assert!(
                    contains_visible_text(footer_rows[0], expected),
                    "{width}x{height} missing {expected} from the one-line footer\n{text}"
                );
            }
            assert!(
                contains_visible_text(&text, app.job_messages().footer_refresh),
                "{width}x{height}\n{text}"
            );
            assert!(
                contains_visible_text(&text, "VISIBLE"),
                "{width}x{height}\n{text}"
            );
            assert!(
                contains_visible_text(&text, "10:00:00 UTC"),
                "{width}x{height} missing exact start time\n{text}"
            );
            if width == 100 {
                for expected in ["#a001", "#b002", app.job_messages().footer_horizontal] {
                    assert!(
                        contains_visible_text(&text, expected),
                        "missing {expected}\n{text}"
                    );
                }
                assert!(!contains_visible_text(&text, "PREFIX00"));
            }
        }
    }

    #[test]
    fn jobs_footer_keeps_essential_keys_on_one_minimum_width_row_in_every_language() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);

        for language in ALL_LANGUAGES {
            app.settings.language = Some(language.code().to_string());
            app.language = language;
            let footer = jobs_footer(&app, 38);
            assert!(!footer.contains('\n'), "{}: {footer}", language.code());
            assert!(
                Line::from(footer.as_str()).width() <= 38,
                "{}: {footer}",
                language.code()
            );
            for key in ["↑↓", "Enter", "R"] {
                assert!(
                    footer.contains(key),
                    "{} missing {key}: {footer}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn jobs_output_renders_residual_controls_as_visible_text() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Jobs;
        app.jobs_state = JobsState::ready(vec![JobSummary {
            id: "j-000001".to_string(),
            command: "printf controls".to_string(),
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
        }]);
        app.jobs_detail.job_id = Some("j-000001".to_string());
        app.jobs_detail.tail.lines = vec!["bell:\u{7}\ttab:\u{8}done".to_string()];

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);

        assert!(text.contains(r"bell:\u{7}\ttab:\u{8}done"), "{text}");
        let buffer = terminal.backend().buffer();
        for cell in buffer.content() {
            assert!(
                cell.symbol()
                    .chars()
                    .all(|character| !character.is_control()),
                "raw control reached the terminal buffer: {:?}",
                cell.symbol()
            );
        }
    }

    #[test]
    fn config_viewport_renders_all_languages_without_breadcrumb_collapse() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();

        for language in ALL_LANGUAGES {
            let mut app = App::for_test(paths.clone(), executable.clone());
            app.settings.language = Some(language.code().to_string());
            app.language = Language::parse(language.code()).unwrap();
            app.screen = Screen::Config;
            app.config_cursor = ConfigCursor::default().next();

            for (width, height) in [(100, 30), (52, 18), (40, 10), (39, 8)] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                let text = buffer_text(&terminal);

                if height >= 21 && width >= 52 {
                    for expected in [
                        app.messages().menu_config,
                        app.messages().config_title,
                        app.messages().extensions_title,
                        app.messages().tier_label,
                        "read",
                        "grep",
                        "glob",
                        "run",
                        "job_output",
                        app.messages().fastshell_label,
                    ] {
                        assert!(
                            contains_visible_text(&text, expected),
                            "{} missing {expected} at {width}x{height}\n{text}",
                            language.code()
                        );
                    }
                    assert!(text.contains('├'), "{}\n{text}", language.code());
                    assert!(text.contains('└'), "{}\n{text}", language.code());
                } else if width >= 40 {
                    assert!(
                        contains_visible_text(&text, "read"),
                        "{} selected child at {width}x{height}\n{text}",
                        language.code()
                    );
                    assert!(
                        contains_visible_text(&text, app.messages().config_more_below),
                        "{} lower viewport marker at {width}x{height}\n{text}",
                        language.code()
                    );
                } else {
                    assert!(text.contains("40×9"), "{}\n{text}", language.code());
                }
            }
        }
    }

    #[test]
    fn config_viewport_follows_focus_and_keeps_detail_below_the_list() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("source");
        std::fs::write(&executable, b"binary").unwrap();
        let mut app = App::for_test(paths, executable);
        app.settings.language = Some("en".to_string());
        app.language = Language::En;
        app.screen = Screen::Config;

        let backend = TestBackend::new(52, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        app.config_cursor = ConfigCursor::default().next();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let top = buffer_text(&terminal);
        assert!(contains_visible_text(&top, "read"));
        assert!(contains_visible_text(
            &top,
            app.messages().config_more_below
        ));
        assert!(!contains_visible_text(
            &top,
            app.messages().config_more_above
        ));

        app.config_cursor = ConfigCursor::default();
        for _ in 0..4 {
            app.config_cursor = app.config_cursor.next();
        }
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::RunBudget);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let middle = buffer_text(&terminal);
        assert!(contains_visible_text(
            &middle,
            app.messages().config_more_above
        ));
        assert!(contains_visible_text(
            &middle,
            app.messages().config_more_below
        ));
        let run_row = row_containing(&terminal, "run").unwrap();
        let detail_row = row_containing(&terminal, "Runs a foreground").unwrap();
        assert!(run_row < detail_row, "{middle}");

        while app.config_cursor.entry().item != ConfigItemId::JobListLimit {
            app.config_cursor = app.config_cursor.next();
        }
        assert_eq!(app.config_cursor.entry().item, ConfigItemId::JobListLimit);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let bottom = buffer_text(&terminal);
        assert!(contains_visible_text(
            &bottom,
            app.job_messages().job_list_limit_label
        ));
        assert!(contains_visible_text(
            &bottom,
            app.messages().config_more_above
        ));
        assert!(!contains_visible_text(
            &bottom,
            app.messages().config_more_below
        ));
    }

    #[test]
    fn status_renders_pass_info_and_fail_as_distinct_user_visible_states() {
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
                    detail: "fastctx is ready".to_string(),
                    remedy: None,
                },
                DoctorCheck {
                    name: "Codex profile",
                    status: DoctorCheckStatus::Info,
                    detail: "Apply will create it".to_string(),
                    remedy: None,
                },
                DoctorCheck {
                    name: "Applied state",
                    status: DoctorCheckStatus::Fail,
                    detail: "Drift detected".to_string(),
                    remedy: Some("Run fastctx apply".to_string()),
                },
            ],
        });
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        theme::with_test_mode(ColorMode::TrueColor, || {
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        });

        let text = buffer_text(&terminal);
        for expected in [
            "fastctx is ready",
            "Apply will create it",
            "Drift detected",
            "Run fastctx apply",
            "~/.codex",
        ] {
            assert!(
                contains_visible_text(&text, expected),
                "missing {expected}\n{text}"
            );
        }
        let palette = Theme::from_mode(ColorMode::TrueColor);
        assert_eq!(symbol_color(&terminal, "✓"), palette.success);
        assert_eq!(symbol_color(&terminal, "○"), palette.muted);
        assert_eq!(symbol_color(&terminal, "×"), palette.danger);
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
