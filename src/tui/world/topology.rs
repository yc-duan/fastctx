//! The World drawn as a picture: machines around the hub the user runs, joined by their links.
//!
//! Three arrangements cover every terminal: a star when a handful of machines and the space for
//! them exist, a tree with a trunk and curved branches when there are many, and a plain text
//! tree when the area is too small to draw at all. Presence is carried by the card's symbol and
//! by the stroke of its connector (solid for online, dashed for offline) before colour is
//! considered, so the picture survives sixteen colours and monochrome.

use super::i18n::WorldMessages;
use super::model::{LinkTone, Presence};
use super::raster::{Dot, Stroke, Surface};
use crate::tui::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub(super) struct Machine {
    pub(super) name: String,
    pub(super) presence: Presence,
    pub(super) system: String,
    pub(super) is_self: bool,
    /// Round trip to the hub as the member reports it.
    pub(super) rtt_ms: Option<u32>,
    /// "3 min ago" for machines that are not online.
    pub(super) last_seen: String,
    pub(super) activity: Option<String>,
}

pub(super) struct Hub {
    pub(super) address: String,
    /// This machine's own link; absent when the node is not running.
    pub(super) link: Option<LinkTone>,
}

pub(super) struct Topology {
    pub(super) hub: Hub,
    pub(super) machines: Vec<Machine>,
    pub(super) selected: Option<usize>,
}

/// Machines this many or fewer are shown as a star when the area allows.
const STAR_LIMIT: usize = 6;
const CARD_ROWS: i32 = 4;
const HUB_ROWS: i32 = 3;
const MIN_CARD_COLUMNS: i32 = 16;
const MAX_CARD_COLUMNS: i32 = 24;

struct Palette {
    machine_fill: Option<Color>,
    hub_fill: Option<Color>,
    outline: Option<Color>,
    text: Color,
    faint: Color,
}

fn palette() -> Palette {
    if theme::monochrome() {
        Palette {
            machine_fill: None,
            hub_fill: None,
            outline: Some(Color::Reset),
            text: Color::Reset,
            faint: Color::Reset,
        }
    } else {
        Palette {
            machine_fill: Some(theme::surface()),
            hub_fill: Some(theme::emphasis()),
            outline: None,
            text: theme::fg(),
            faint: theme::muted(),
        }
    }
}

fn width_of(text: &str) -> i32 {
    Span::raw(text).width() as i32
}

impl Topology {
    fn card_columns(&self) -> i32 {
        let widest = self
            .machines
            .iter()
            .map(|machine| width_of(&machine.name) + 4)
            .max()
            .unwrap_or(0);
        widest.clamp(MIN_CARD_COLUMNS, MAX_CARD_COLUMNS)
    }
    fn hub_columns(&self) -> i32 {
        (width_of(&self.hub.address) + 4).clamp(MIN_CARD_COLUMNS + 2, 34)
    }
    fn stroke(&self, machine: &Machine) -> (Color, Stroke) {
        if machine.is_self {
            return match self.hub.link {
                Some(tone) => (
                    tone.color(),
                    if tone == LinkTone::Connected {
                        Stroke::Solid
                    } else {
                        Stroke::Dashed
                    },
                ),
                None => (theme::danger(), Stroke::Dashed),
            };
        }
        match machine.presence {
            Presence::Online => (theme::success(), Stroke::Solid),
            Presence::Offline => (theme::muted(), Stroke::Dashed),
            Presence::Revoked => (theme::danger(), Stroke::Dashed),
            Presence::Unknown => (theme::warning(), Stroke::Dashed),
        }
    }
}

/// A card placed on the cell grid.
#[derive(Clone, Copy, Debug)]
struct Box {
    column: i32,
    row: i32,
    columns: i32,
    rows: i32,
}

impl Box {
    fn overlaps(&self, other: &Box) -> bool {
        self.column < other.column + other.columns + 1
            && other.column < self.column + self.columns + 1
            && self.row < other.row + other.rows
            && other.row < self.row + self.rows
    }
    fn center(&self) -> Dot {
        Dot::new(
            f64::from(self.column * 2 + self.columns),
            f64::from(self.row * 4 + self.rows * 2),
        )
    }
}

/// Renders the picture, falling back to a text tree when the area is too small for cards.
pub(super) fn render(buf: &mut Buffer, area: Rect, topology: &Topology, m: &WorldMessages) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = i32::from(area.width);
    let rows = i32::from(area.height);
    let card = topology.card_columns();
    let hub = topology.hub_columns();
    if let Some(placement) = star(topology, columns, rows, card, hub) {
        draw_star(buf, area, topology, m, &placement);
    } else if columns >= hub.max(card + 6) && rows >= HUB_ROWS + 1 + CARD_ROWS {
        draw_tree(buf, area, topology, m, card, hub);
    } else {
        draw_list(buf, area, topology, m);
    }
}

struct Star {
    hub: Box,
    machines: Vec<Box>,
}

fn star(topology: &Topology, columns: i32, rows: i32, card: i32, hub: i32) -> Option<Star> {
    let count = topology.machines.len();
    if count == 0 || count > STAR_LIMIT {
        return None;
    }
    let hub_box = Box {
        column: (columns - hub) / 2,
        row: (rows - HUB_ROWS) / 2,
        columns: hub,
        rows: HUB_ROWS,
    };
    let radius_x = f64::from((columns - card) / 2 - 1);
    let radius_y = f64::from(rows - CARD_ROWS) / 2.0;
    if radius_x < f64::from(card / 2 + hub / 2 + 3) && count > 1 {
        return None;
    }
    let mut machines = Vec::with_capacity(count);
    for index in 0..count {
        let angle =
            -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * index as f64 / count as f64;
        let center_column = f64::from(columns) / 2.0 + radius_x * angle.cos();
        let center_row = f64::from(rows) / 2.0 + radius_y * angle.sin();
        let placed = Box {
            column: (center_column - f64::from(card) / 2.0).round() as i32,
            row: (center_row - f64::from(CARD_ROWS) / 2.0).round() as i32,
            columns: card,
            rows: CARD_ROWS,
        };
        if placed.column < 0
            || placed.row < 0
            || placed.column + card > columns
            || placed.row + CARD_ROWS > rows
            || placed.overlaps(&hub_box)
            || machines.iter().any(|other| placed.overlaps(other))
        {
            return None;
        }
        machines.push(placed);
    }
    Some(Star {
        hub: hub_box,
        machines,
    })
}

fn draw_star(
    buf: &mut Buffer,
    area: Rect,
    topology: &Topology,
    m: &WorldMessages,
    placement: &Star,
) {
    let colors = palette();
    let mut surface = Surface::new(area.width, area.height, super::raster::glyphs());
    let hub_center = placement.hub.center();
    for (machine, placed) in topology.machines.iter().zip(&placement.machines) {
        let (color, stroke) = topology.stroke(machine);
        let center = placed.center();
        surface.gradient(hub_center, center, colors.faint, color, stroke);
        if let Some(rtt) = machine
            .rtt_ms
            .filter(|_| machine.presence == Presence::Online)
        {
            let label = format!(" {rtt} ms ");
            let width = width_of(&label);
            let mid_column = ((hub_center.x + center.x) / 4.0).round() as i32 - width / 2;
            let mid_row = ((hub_center.y + center.y) / 8.0).round() as i32;
            let clear = |b: &Box| {
                !(mid_row >= b.row - 1 && mid_row <= b.row + b.rows)
                    || mid_column + width < b.column
                    || mid_column > b.column + b.columns
            };
            if clear(&placement.hub) && clear(placed) {
                surface.label(
                    mid_column,
                    mid_row,
                    label,
                    Style::default().fg(colors.faint).bg(theme::bg()),
                    width,
                );
            }
        }
    }
    draw_hub_card(&mut surface, &colors, &placement.hub, &topology.hub, m);
    for (index, (machine, placed)) in topology
        .machines
        .iter()
        .zip(&placement.machines)
        .enumerate()
    {
        draw_machine_card(
            &mut surface,
            &colors,
            placed,
            machine,
            m,
            topology.selected == Some(index),
        );
    }
    surface.render(area, buf);
}

fn draw_tree(
    buf: &mut Buffer,
    area: Rect,
    topology: &Topology,
    m: &WorldMessages,
    card: i32,
    hub: i32,
) {
    let colors = palette();
    let columns = i32::from(area.width);
    let rows = i32::from(area.height);
    let mut surface = Surface::new(area.width, area.height, super::raster::glyphs());
    let hub_box = Box {
        column: 0,
        row: 0,
        columns: hub,
        rows: HUB_ROWS,
    };
    let trunk_column = 2;
    let first_column = 5;
    let per_row = ((columns - first_column + 1) / (card + 1)).max(1) as usize;
    let mut placed = Vec::new();
    let mut row = HUB_ROWS + 1;
    let mut hidden = 0usize;
    for (index, _) in topology.machines.iter().enumerate() {
        let slot = index % per_row;
        if slot == 0 && index > 0 {
            row += CARD_ROWS + 1;
        }
        if row + CARD_ROWS > rows {
            hidden = topology.machines.len() - index;
            break;
        }
        placed.push(Box {
            column: first_column + slot as i32 * (card + 1),
            row,
            columns: card,
            rows: CARD_ROWS,
        });
    }
    let trunk = Dot::new(f64::from(trunk_column * 2 + 1), f64::from(HUB_ROWS * 4));
    let mut trunk_end = trunk;
    for (machine, placed) in topology.machines.iter().zip(&placed) {
        let (color, stroke) = topology.stroke(machine);
        let mid = Dot::new(f64::from(placed.column * 2), placed.center().y);
        if placed.column == first_column {
            let junction = Dot::new(trunk.x, mid.y - 6.0);
            trunk_end = junction;
            surface.curve(junction, Dot::new(trunk.x, mid.y), mid, color, stroke);
        } else {
            let from = Dot::new(f64::from(placed.column * 2 - 2), mid.y);
            surface.line(from, mid, color, stroke);
        }
    }
    surface.line(trunk, trunk_end, colors.faint, Stroke::Solid);
    draw_hub_card(&mut surface, &colors, &hub_box, &topology.hub, m);
    for (index, (machine, placed)) in topology.machines.iter().zip(&placed).enumerate() {
        draw_machine_card(
            &mut surface,
            &colors,
            placed,
            machine,
            m,
            topology.selected == Some(index),
        );
    }
    if hidden > 0 {
        surface.label(
            first_column,
            rows - 1,
            format!("+{hidden}  ({})", m.machines),
            Style::default().fg(colors.faint),
            columns - first_column,
        );
    }
    surface.render(area, buf);
}

fn draw_hub_card(
    surface: &mut Surface,
    colors: &Palette,
    placed: &Box,
    hub: &Hub,
    m: &WorldMessages,
) {
    surface.panel(
        placed.column * 2,
        placed.row * 4,
        placed.columns * 2,
        placed.rows * 4,
        colors.hub_fill,
        colors.outline,
    );
    let inner = placed.columns - 2;
    let title = Style::default()
        .fg(theme::accent())
        .add_modifier(Modifier::BOLD);
    surface.label(
        placed.column + 1,
        placed.row,
        format!("◆ {}", m.hub_title),
        title,
        inner,
    );
    surface.label(
        placed.column + 1,
        placed.row + 1,
        hub.address.clone(),
        Style::default().fg(colors.text),
        inner,
    );
    let (symbol, label, color) = match hub.link {
        Some(tone) => (tone.symbol(), tone.label(m), tone.color()),
        None => ("?", m.unknown, theme::warning()),
    };
    surface.label(
        placed.column + 1,
        placed.row + 2,
        format!("{symbol} {label}"),
        Style::default().fg(color),
        inner,
    );
}

fn draw_machine_card(
    surface: &mut Surface,
    colors: &Palette,
    placed: &Box,
    machine: &Machine,
    m: &WorldMessages,
    selected: bool,
) {
    let (x, y, w, h) = (
        placed.column * 2,
        placed.row * 4,
        placed.columns * 2,
        placed.rows * 4,
    );
    surface.panel(x, y, w, h, colors.machine_fill, colors.outline);
    if selected {
        surface.frame(x - 1, y - 1, w + 2, h + 2, theme::accent());
    }
    let inner = placed.columns - 2;
    let name = format!(
        "{}{}",
        if selected { "› " } else { "" },
        if machine.is_self {
            format!("★ {}", machine.name)
        } else {
            machine.name.clone()
        }
    );
    surface.label(
        placed.column + 1,
        placed.row,
        name,
        Style::default()
            .fg(if machine.is_self || selected {
                theme::accent()
            } else {
                colors.text
            })
            .add_modifier(Modifier::BOLD),
        inner,
    );
    surface.label(
        placed.column + 1,
        placed.row + 1,
        machine.system.clone(),
        Style::default().fg(colors.faint),
        inner,
    );
    let presence = match (machine.presence, machine.rtt_ms) {
        (Presence::Online, Some(rtt)) => {
            format!("● {} · {rtt} ms", machine.presence.label(m))
        }
        (Presence::Online, None) => format!("● {}", machine.presence.label(m)),
        (presence, _) => format!(
            "{} {} · {}",
            presence.symbol(),
            presence.label(m),
            machine.last_seen
        ),
    };
    surface.label(
        placed.column + 1,
        placed.row + 2,
        presence,
        Style::default().fg(machine.presence.color()),
        inner,
    );
    if let Some(activity) = &machine.activity {
        surface.label(
            placed.column + 1,
            placed.row + 3,
            activity.clone(),
            Style::default().fg(colors.faint),
            inner,
        );
    }
}

/// The welcome page's illustration: a few machines meeting at a hub, so the idea lands before
/// a single word of the form. Nothing here is live data.
pub(super) fn sketch(buf: &mut Buffer, area: Rect, m: &WorldMessages) {
    let columns = i32::from(area.width);
    let rows = i32::from(area.height);
    if rows < 6 {
        return;
    }
    let colors = palette();
    let machine_label = m.sketch_machine;
    let hub_label = format!("◆ {}", m.sketch_hub);
    let card = width_of(machine_label) + 4;
    let count = ((columns + 2) / (card + 2)).clamp(0, 3);
    if count < 2 {
        return;
    }
    let total = count * card + (count - 1) * 2;
    let left = (columns - total) / 2;
    let hub_width = width_of(&hub_label) + 4;
    let hub_left = (columns - hub_width) / 2;
    let hub_top = rows - 2;
    let mut surface = Surface::new(area.width, area.height, super::raster::glyphs());
    let hub_center_x = f64::from(hub_left * 2 + hub_width);
    for index in 0..count {
        let column = left + index * (card + 2);
        let from = Dot::new(f64::from(column * 2 + card), 8.0);
        let to = Dot::new(hub_center_x, f64::from(hub_top * 4 - 1));
        surface.curve(
            from,
            Dot::new(hub_center_x, from.y),
            to,
            theme::success(),
            Stroke::Solid,
        );
    }
    for index in 0..count {
        let column = left + index * (card + 2);
        surface.panel(
            column * 2,
            0,
            card * 2,
            8,
            colors.machine_fill,
            colors.outline,
        );
        surface.label(
            column + 2,
            0,
            machine_label,
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
            card - 4,
        );
        surface.label(
            column + 2,
            1,
            format!("● {}", m.online),
            Style::default().fg(theme::success()),
            card - 4,
        );
    }
    surface.panel(
        hub_left * 2,
        hub_top * 4,
        hub_width * 2,
        8,
        colors.hub_fill,
        colors.outline,
    );
    surface.label(
        hub_left + 2,
        hub_top,
        hub_label,
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
        hub_width - 4,
    );
    surface.label(
        hub_left + 2,
        hub_top + 1,
        "fastctx hub serve",
        Style::default().fg(colors.faint),
        hub_width - 4,
    );
    surface.render(area, buf);
}

fn draw_list(buf: &mut Buffer, area: Rect, topology: &Topology, m: &WorldMessages) {
    let (symbol, label, color) = match topology.hub.link {
        Some(tone) => (tone.symbol(), tone.label(m), tone.color()),
        None => ("?", m.unknown, theme::warning()),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("◆ {} ", m.hub_title),
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(topology.hub.address.clone()),
        Span::styled(format!("  {symbol} {label}"), Style::default().fg(color)),
    ])];
    let count = topology.machines.len();
    for (index, machine) in topology.machines.iter().enumerate() {
        let branch = if index + 1 == count { "└ " } else { "├ " };
        let selected = topology.selected == Some(index);
        let tail = match (machine.presence, machine.rtt_ms) {
            (Presence::Online, Some(rtt)) => format!("{rtt} ms"),
            (Presence::Online, None) => machine.presence.label(m).to_string(),
            _ => format!("{} · {}", machine.presence.label(m), machine.last_seen),
        };
        let mut style = Style::default();
        if selected {
            style = style.fg(theme::accent()).add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(vec![
            Span::styled(branch, Style::default().fg(theme::muted())),
            Span::styled(
                format!("{} ", machine.presence.symbol()),
                Style::default().fg(machine.presence.color()),
            ),
            Span::styled(
                format!(
                    "{}{}  {}  {tail}",
                    if machine.is_self { "★ " } else { "" },
                    machine.name,
                    machine.system
                ),
                style,
            ),
        ]));
    }
    Paragraph::new(lines).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::i18n::Language;

    fn sample(count: usize, selected: Option<usize>) -> Topology {
        Topology {
            hub: Hub {
                address: "hub.example:443".into(),
                link: Some(LinkTone::Connected),
            },
            machines: (0..count)
                .map(|index| Machine {
                    name: format!("machine-{index}"),
                    presence: if index % 3 == 1 {
                        Presence::Offline
                    } else {
                        Presence::Online
                    },
                    system: "Linux · x86_64".into(),
                    is_self: index == 0,
                    rtt_ms: Some(12 + index as u32),
                    last_seen: "3 min ago".into(),
                    activity: Some("Idle".into()),
                })
                .collect(),
            selected,
        }
    }

    fn text(buf: &Buffer) -> String {
        buf.content.iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn every_arrangement_renders_without_panicking_and_names_each_machine() {
        let m = super::super::i18n::messages(Language::En);
        for count in [1, 2, 3, 6, 7, 20] {
            for (width, height) in [(38, 6), (52, 12), (78, 17), (120, 34)] {
                let area = Rect::new(0, 0, width, height);
                let mut buf = Buffer::empty(area);
                render(&mut buf, area, &sample(count, Some(0)), m);
                let rendered = text(&buf);
                assert!(
                    rendered.contains("machine-0"),
                    "{count} machines at {width}x{height}: {rendered}"
                );
            }
        }
    }

    #[test]
    fn a_small_world_becomes_a_star_and_a_large_one_a_tree() {
        assert!(star(&sample(3, None), 100, 20, 20, 22).is_some());
        assert!(star(&sample(7, None), 200, 60, 20, 22).is_none());
        assert!(star(&sample(4, None), 50, 20, 20, 22).is_none());
    }
}
