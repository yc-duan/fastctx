//! The Overview picture: the hub above its machines, joined by thin rounded lines, with the
//! selected machine lifted one shade. Everything is box drawing and block elements, the two
//! sets every terminal renders, so the drawing looks the same in a console and a GPU terminal.

use super::i18n::WorldMessages;
use super::model::{LinkTone, Presence};
use super::widgets::{card, clipped, padded, safe, selection_style, width};
use crate::tui::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

pub(super) struct Machine {
    pub(super) name: String,
    pub(super) presence: Presence,
    pub(super) system: String,
    pub(super) is_self: bool,
    pub(super) rtt_ms: Option<u32>,
    pub(super) last_seen: String,
}

pub(super) struct Hub {
    pub(super) address: String,
    /// Only this machine's own connection is known; it does not establish hub-wide health.
    pub(super) link: Option<LinkTone>,
}

pub(super) struct Topology {
    pub(super) hub: Hub,
    pub(super) machines: Vec<Machine>,
    pub(super) selected: Option<usize>,
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Lines are accumulated as direction masks and resolved to one glyph per cell, so junctions
/// come out right no matter in which order the segments were drawn.
struct Wires {
    columns: u16,
    cells: Vec<(u8, Option<Color>, bool)>,
}

impl Wires {
    fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns,
            cells: vec![(0, None, false); usize::from(columns) * usize::from(rows)],
        }
    }
    fn cell(&mut self, x: u16, y: u16) -> Option<&mut (u8, Option<Color>, bool)> {
        if x >= self.columns {
            return None;
        }
        self.cells
            .get_mut(usize::from(y) * usize::from(self.columns) + usize::from(x))
    }
    fn hline(&mut self, y: u16, x0: u16, x1: u16, color: Color) {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        for x in x0..=x1 {
            if let Some(cell) = self.cell(x, y) {
                if x > x0 {
                    cell.0 |= LEFT;
                }
                if x < x1 {
                    cell.0 |= RIGHT;
                }
                cell.1 = Some(color);
            }
        }
    }
    /// A vertical run that keeps the colour of any horizontal it crosses.
    fn vline(&mut self, x: u16, y0: u16, y1: u16, color: Color, dashed: bool) {
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        for y in y0..=y1 {
            if let Some(cell) = self.cell(x, y) {
                if y > y0 {
                    cell.0 |= UP;
                }
                if y < y1 {
                    cell.0 |= DOWN;
                }
                if cell.1.is_none() {
                    cell.1 = Some(color);
                }
                cell.2 |= dashed;
            }
        }
    }
    /// Marks a cell as continuing a line that arrives from the row above.
    fn enter_from_above(&mut self, x: u16, y: u16, color: Color, dashed: bool) {
        if let Some(cell) = self.cell(x, y) {
            cell.0 |= UP;
            if cell.1.is_none() {
                cell.1 = Some(color);
            }
            cell.2 |= dashed;
        }
    }
    fn render(&self, buf: &mut Buffer, area: Rect) {
        for (index, (mask, color, dashed)) in self.cells.iter().enumerate() {
            if *mask == 0 {
                continue;
            }
            let x = area.x + (index % usize::from(self.columns)) as u16;
            let y = area.y + (index / usize::from(self.columns)) as u16;
            if x >= area.right() || y >= area.bottom() {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(glyph(*mask, *dashed))
                    .set_fg(color.unwrap_or_else(theme::border));
            }
        }
    }
}

const VERTICAL: u8 = UP | DOWN;
const HORIZONTAL: u8 = LEFT | RIGHT;
const TOP_LEFT: u8 = DOWN | RIGHT;
const TOP_RIGHT: u8 = DOWN | LEFT;
const BOTTOM_LEFT: u8 = UP | RIGHT;
const BOTTOM_RIGHT: u8 = UP | LEFT;
const TEE_DOWN: u8 = LEFT | RIGHT | DOWN;
const TEE_UP: u8 = LEFT | RIGHT | UP;
const TEE_RIGHT: u8 = UP | DOWN | RIGHT;
const TEE_LEFT: u8 = UP | DOWN | LEFT;

fn glyph(mask: u8, dashed: bool) -> &'static str {
    match mask {
        UP | DOWN | VERTICAL => {
            if dashed {
                "┆"
            } else {
                "│"
            }
        }
        LEFT | RIGHT | HORIZONTAL => "─",
        TOP_LEFT => "╭",
        TOP_RIGHT => "╮",
        BOTTOM_LEFT => "╰",
        BOTTOM_RIGHT => "╯",
        TEE_DOWN => "┬",
        TEE_UP => "┴",
        TEE_RIGHT => "├",
        TEE_LEFT => "┤",
        _ => "┼",
    }
}

/// Row budgets for one layout tier: the hub card, the link rows between hub and cards, and a
/// machine card. The full tier pads its cards; the tight tier keeps the same picture in fewer rows.
#[derive(Clone, Copy)]
struct Tier {
    hub: u16,
    link: u16,
    card: u16,
    pad: u16,
}

const FULL: Tier = Tier {
    hub: 4,
    link: 3,
    card: 5,
    pad: 1,
};
const TIGHT: Tier = Tier {
    hub: 2,
    link: 2,
    card: 3,
    pad: 0,
};
/// Rows between two rows of cards: a bus and the drops into the next row.
const GAP: u16 = 2;

struct Plan {
    tier: Tier,
    card_width: u16,
    columns: u16,
    rows: u16,
    hub_width: u16,
    left: u16,
    rail: Option<u16>,
    /// A page counter needs its own row so it never sits on a card.
    counter: bool,
}

impl Plan {
    fn height_for(&self, rows: u16) -> u16 {
        self.tier.hub
            + self.tier.link
            + self.tier.card
            + (rows.max(1) - 1) * (self.tier.card + GAP)
            + u16::from(self.counter)
    }
    fn capacity(&self) -> usize {
        usize::from(self.columns) * usize::from(self.rows)
    }
    /// The machines this page shows, given which one is selected.
    fn page(&self, topology: &Topology) -> (usize, usize) {
        let selected = topology
            .selected
            .unwrap_or(0)
            .min(topology.machines.len().saturating_sub(1));
        let first = selected / self.capacity() * self.capacity();
        (
            first,
            (first + self.capacity()).min(topology.machines.len()),
        )
    }
    /// Rows the current page actually fills; the last page is usually shorter than the plan.
    fn page_rows(&self, topology: &Topology) -> u16 {
        let (first, last) = self.page(topology);
        (last - first).div_ceil(usize::from(self.columns)).max(1) as u16
    }
}

fn machine_name(machine: &Machine, m: &WorldMessages) -> (String, String) {
    (
        safe(&machine.name),
        if machine.is_self {
            m.this_machine.to_string()
        } else {
            String::new()
        },
    )
}

fn machine_status(machine: &Machine, m: &WorldMessages) -> String {
    let detail = if machine.presence == Presence::Online {
        machine
            .rtt_ms
            .map(|rtt| format!("{rtt} ms"))
            .unwrap_or_default()
    } else if machine.presence == Presence::Offline {
        machine.last_seen.clone()
    } else {
        String::new()
    };
    let mut status = format!(
        "{} {}",
        machine.presence.symbol(),
        machine.presence.label(m)
    );
    if !detail.is_empty() {
        status.push_str(" · ");
        status.push_str(&detail);
    }
    status
}

fn plan(topology: &Topology, m: &WorldMessages, width: u16, available: u16) -> Option<Plan> {
    if topology.machines.is_empty() || width < 24 {
        return None;
    }
    let content = topology
        .machines
        .iter()
        .flat_map(|machine| {
            let (name, tag) = machine_name(machine, m);
            let title = if tag.is_empty() {
                name
            } else {
                format!("{name}  {tag}")
            };
            [title, safe(&machine.system), machine_status(machine, m)]
        })
        .map(|text| self::width(&text))
        .max()
        .unwrap_or(16);
    let card_width = (content as u16 + 4)
        .clamp(20, 40)
        .min(width.saturating_sub(2));
    let total = topology.machines.len();
    let mut columns = ((width + 2) / (card_width + 2)).max(1).min(total as u16);
    let mut rows_needed = total.div_ceil(usize::from(columns)) as u16;
    let mut rail = None;
    if rows_needed > 1 {
        columns = (width.saturating_sub(2) / (card_width + 2))
            .max(1)
            .min(total as u16);
        rows_needed = total.div_ceil(usize::from(columns)) as u16;
    }
    let grid_width = columns * (card_width + 2) - 2;
    let left = (width - grid_width) / 2;
    if rows_needed > 1 {
        rail = Some(left.saturating_sub(2));
    }
    for tier in [FULL, TIGHT] {
        let base = tier.hub + tier.link + tier.card;
        if available >= base {
            let mut rows = (1 + (available - base) / (tier.card + GAP)).min(rows_needed);
            let mut counter = rows < rows_needed;
            if counter && available < base + (rows - 1) * (tier.card + GAP) + 1 {
                if rows > 1 {
                    rows -= 1;
                } else if available == base {
                    counter = false;
                }
            }
            let hub_width = (self::width(&safe(&topology.hub.address)) as u16 + 8)
                .clamp(24, 48)
                .min(width);
            return Some(Plan {
                tier,
                card_width,
                columns,
                rows,
                hub_width,
                left,
                rail,
                counter,
            });
        }
    }
    None
}

/// How many rows the picture wants for this width, at most `available`.
pub(super) fn preferred_height(
    topology: &Topology,
    m: &WorldMessages,
    width: u16,
    available: u16,
) -> u16 {
    match plan(topology, m, width, available) {
        Some(plan) => plan.height_for(plan.page_rows(topology)).min(available),
        None => (topology.machines.len() as u16 + 1).min(available),
    }
}

pub(super) fn render(buf: &mut Buffer, area: Rect, topology: &Topology, m: &WorldMessages) {
    if area.is_empty() {
        return;
    }
    let Some(plan) = plan(topology, m, area.width, area.height) else {
        compact(buf, area, topology, m);
        return;
    };
    let tier = plan.tier;
    let (first, last) = plan.page(topology);
    let visible = &topology.machines[first..last];
    let rows = plan.page_rows(topology);
    // The rail only exists to reach a second row; a one-row page hangs straight off the hub.
    let rail = plan.rail.filter(|_| rows > 1);
    let hub_left = (area.width - plan.hub_width) / 2;
    let hub_center = hub_left + plan.hub_width / 2;
    let in_row = |row: u16| {
        (visible.len() - usize::from(row) * usize::from(plan.columns))
            .min(usize::from(plan.columns)) as u16
    };
    // A single row hangs straight off the hub, so it is centred on the hub however many cards
    // it holds; the rows under a rail stay on the grid the rail runs down.
    let row_left = |row: u16| {
        if rail.is_some() {
            return plan.left;
        }
        let span = in_row(row) * (plan.card_width + 2) - 2;
        hub_center
            .saturating_sub(span / 2)
            .min(area.width.saturating_sub(span))
    };
    let center = |row: u16, column: u16| {
        row_left(row) + column * (plan.card_width + 2) + plan.card_width / 2
    };
    let hub_bottom = tier.hub;
    let bus_row = |row: u16| {
        if row == 0 {
            hub_bottom + tier.link - 2
        } else {
            hub_bottom + tier.link + row * (tier.card + GAP) - 2
        }
    };
    let cards_row = |row: u16| bus_row(row) + 2;
    let mut wires = Wires::new(area.width, area.height);
    let border = theme::border();
    for row in 0..rows {
        let last_center = center(row, in_row(row) - 1);
        let (start, end) = if row == 0 {
            (
                rail.unwrap_or(center(0, 0).min(hub_center)),
                last_center.max(hub_center),
            )
        } else {
            (rail.unwrap_or(center(row, 0)), last_center)
        };
        wires.hline(bus_row(row), start, end, border);
    }
    let link_color = topology.hub.link.map_or(theme::warning(), LinkTone::color);
    for y in hub_bottom..=bus_row(0) {
        wires.enter_from_above(hub_center, y, link_color, false);
        if y < bus_row(0) {
            wires.vline(hub_center, y, y + 1, link_color, false);
        }
    }
    if let Some(rail) = rail {
        wires.vline(rail, bus_row(0), bus_row(rows - 1), border, false);
    }
    for (index, machine) in visible.iter().enumerate() {
        let column = index as u16 % plan.columns;
        let row = index as u16 / plan.columns;
        let x = center(row, column);
        let dashed = machine.presence != Presence::Online;
        wires.vline(
            x,
            bus_row(row),
            bus_row(row) + 1,
            machine.presence.color(),
            dashed,
        );
    }
    wires.render(buf, area);
    hub_card(
        buf,
        Rect::new(area.x + hub_left, area.y, plan.hub_width, tier.hub),
        &topology.hub,
        tier,
        m,
    );
    for (index, machine) in visible.iter().enumerate() {
        let column = index as u16 % plan.columns;
        let row = index as u16 / plan.columns;
        let rect = Rect::new(
            area.x + row_left(row) + column * (plan.card_width + 2),
            area.y + cards_row(row),
            plan.card_width,
            tier.card,
        );
        if rect.bottom() > area.bottom() {
            break;
        }
        machine_card(
            buf,
            rect,
            machine,
            topology.selected == Some(first + index),
            tier,
            m,
        );
    }
    if plan.counter && topology.machines.len() > plan.capacity() {
        let count = format!("{}–{} / {}  ↑↓", first + 1, last, topology.machines.len());
        let count_width = self::width(&count) as u16;
        // Directly under the last card of this page, not at the bottom of the room the plan
        // asked for: a short last page would otherwise leave the counter floating.
        let y = area.y + cards_row(rows - 1) + tier.card;
        if count_width < area.width && y < area.bottom() {
            buf.set_stringn(
                area.right() - count_width,
                y,
                &count,
                usize::from(count_width),
                Style::default().fg(theme::muted()),
            );
        }
    }
}

/// Outlined cards lose two rows to their frame, so padding is dropped before content is.
fn padding(inner: Rect, tier: Tier, rows: u16) -> u16 {
    if inner.height >= rows + 2 * tier.pad {
        tier.pad
    } else {
        0
    }
}

fn hub_card(buf: &mut Buffer, area: Rect, hub: &Hub, tier: Tier, m: &WorldMessages) {
    let inner = card(buf, area, false);
    let y = inner.y + padding(inner, tier, 2);
    let title = format!("◆ {}", m.hub_title);
    buf.set_stringn(
        inner.x,
        y,
        &title,
        usize::from(inner.width),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );
    if let Some(link) = hub.link {
        let state = format!("{} {}", link.symbol(), link.label(m));
        let state_width = self::width(&state) as u16;
        if self::width(&title) as u16 + state_width + 2 <= inner.width {
            buf.set_stringn(
                inner.right() - state_width,
                y,
                &state,
                usize::from(state_width),
                Style::default().fg(link.color()),
            );
        }
    }
    if y + 1 < inner.bottom() {
        buf.set_stringn(
            inner.x,
            y + 1,
            clipped(&safe(&hub.address), usize::from(inner.width)),
            usize::from(inner.width),
            Style::default().fg(theme::fg()),
        );
    }
}

fn machine_card(
    buf: &mut Buffer,
    area: Rect,
    machine: &Machine,
    selected: bool,
    tier: Tier,
    m: &WorldMessages,
) {
    let inner = card(buf, area, selected);
    let dim = matches!(machine.presence, Presence::Offline | Presence::Revoked);
    let (name, tag) = machine_name(machine, m);
    let y = inner.y + padding(inner, tier, 3);
    let name_style = Style::default()
        .fg(if dim { theme::muted() } else { theme::accent() })
        .add_modifier(Modifier::BOLD);
    // The tag keeps its place; a long name gives way to it rather than pushing it off.
    let name_room = if tag.is_empty() {
        usize::from(inner.width)
    } else {
        usize::from(inner.width).saturating_sub(self::width(&tag) + 2)
    };
    let name = clipped(&name, name_room.max(4));
    buf.set_stringn(inner.x, y, &name, usize::from(inner.width), name_style);
    if !tag.is_empty() {
        let offset = self::width(&name) as u16 + 2;
        if offset < inner.width {
            buf.set_stringn(
                inner.x + offset,
                y,
                &tag,
                usize::from(inner.width - offset),
                Style::default().fg(theme::muted()),
            );
        }
    }
    if y + 1 < inner.bottom() {
        buf.set_stringn(
            inner.x,
            y + 1,
            clipped(&safe(&machine.system), usize::from(inner.width)),
            usize::from(inner.width),
            Style::default().fg(theme::muted()),
        );
    }
    if y + 2 < inner.bottom() {
        buf.set_stringn(
            inner.x,
            y + 2,
            clipped(&machine_status(machine, m), usize::from(inner.width)),
            usize::from(inner.width),
            Style::default().fg(machine.presence.color()),
        );
    }
}

/// The same picture as a list, for terminals too small to draw cards.
fn compact(buf: &mut Buffer, area: Rect, topology: &Topology, m: &WorldMessages) {
    let state = topology
        .hub
        .link
        .map(|link| format!("{} {}", link.symbol(), link.label(m)));
    let hub = format!("◆ {}  {}", m.hub_title, safe(&topology.hub.address));
    let state_width = state.as_ref().map_or(0, |state| self::width(state) as u16);
    buf.set_stringn(
        area.x,
        area.y,
        clipped(
            &hub,
            usize::from(area.width.saturating_sub(state_width + 2)),
        ),
        usize::from(area.width),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );
    if let Some(state) = state
        && state_width < area.width
    {
        buf.set_stringn(
            area.right() - state_width,
            area.y,
            &state,
            usize::from(state_width),
            Style::default().fg(topology.hub.link.map_or(theme::fg(), LinkTone::color)),
        );
    }
    let capacity = usize::from(area.height.saturating_sub(1)).max(1);
    let selected = topology
        .selected
        .unwrap_or(0)
        .min(topology.machines.len().saturating_sub(1));
    let first = selected / capacity * capacity;
    for (slot, machine) in topology
        .machines
        .iter()
        .skip(first)
        .take(capacity)
        .enumerate()
    {
        let y = area.y + 1 + slot as u16;
        if y >= area.bottom() {
            break;
        }
        let focused = topology.selected == Some(first + slot);
        let status = machine_status(machine, m);
        let status_width = self::width(&status) as u16;
        let (name, tag) = machine_name(machine, m);
        let title = if tag.is_empty() {
            name
        } else {
            format!("{name} · {tag}")
        };
        let name_width = usize::from(area.width.saturating_sub(status_width + 4));
        let line = format!(
            "{} {}",
            if focused { "▌" } else { " " },
            padded(&title, name_width)
        );
        let style = if focused {
            selection_style()
        } else {
            Style::default().fg(theme::fg())
        };
        buf.set_stringn(area.x, y, &line, usize::from(area.width), style);
        if status_width < area.width {
            buf.set_stringn(
                area.right() - status_width,
                y,
                &status,
                usize::from(status_width),
                if focused {
                    style
                } else {
                    Style::default().fg(machine.presence.color())
                },
            );
        }
    }
}

/// The welcome page's illustration of what a World is: one hub, several trusted machines.
/// Rows the sketch needs at the current palette; outlined cards cost two more.
pub(super) fn sketch_height() -> u16 {
    if theme::flat() { 11 } else { 8 }
}

pub(super) fn sketch(buf: &mut Buffer, area: Rect, m: &WorldMessages) {
    if area.width < 48 || area.height < sketch_height() {
        return;
    }
    let card_width = ((area.width - 8) / 3 - 2).min(24);
    let grid = card_width * 3 + 4;
    let left = (area.width - grid) / 2;
    let centers = [0, 1, 2].map(|index| left + index * (card_width + 2) + card_width / 2);
    let hub_width = (self::width(m.sketch_hub).max(self::width("fastctx hub serve")) as u16 + 6)
        .min(area.width);
    let hub_left = (area.width - hub_width) / 2;
    let hub_center = hub_left + hub_width / 2;
    // Outlined cards spend two rows on their frame, so the picture grows by them.
    let hub_height = if theme::flat() { 4 } else { 2 };
    let bus = hub_height + 1;
    let mut wires = Wires::new(area.width, area.height);
    wires.hline(
        bus,
        centers[0].min(hub_center),
        centers[2].max(hub_center),
        theme::border(),
    );
    wires.enter_from_above(hub_center, hub_height, theme::border(), false);
    wires.vline(hub_center, hub_height, bus, theme::border(), false);
    for center in centers {
        wires.vline(center, bus, bus + 1, theme::success(), false);
    }
    wires.render(buf, area);
    let hub = Rect::new(area.x + hub_left, area.y, hub_width, hub_height);
    let inner = card(buf, hub, false);
    buf.set_stringn(
        inner.x,
        inner.y,
        format!("◆ {}", m.sketch_hub),
        usize::from(inner.width),
        Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD),
    );
    if inner.height > 1 {
        buf.set_stringn(
            inner.x,
            inner.y + 1,
            "fastctx hub serve",
            usize::from(inner.width),
            Style::default().fg(theme::muted()),
        );
    }
    let cards_y = bus + 2;
    if cards_y + 2 > area.height {
        return;
    }
    let card_height = (area.height - cards_y).min(if theme::flat() { 4 } else { 3 });
    for (index, name) in ["Windows", "macOS", "Linux"].into_iter().enumerate() {
        let rect = Rect::new(
            area.x + left + index as u16 * (card_width + 2),
            area.y + cards_y,
            card_width,
            card_height,
        );
        let inner = card(buf, rect, false);
        buf.set_stringn(
            inner.x,
            inner.y,
            name,
            usize::from(inner.width),
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD),
        );
        if inner.y + 1 < inner.bottom() {
            buf.set_stringn(
                inner.x,
                inner.y + 1,
                clipped(m.sketch_machine, usize::from(inner.width)),
                usize::from(inner.width),
                Style::default().fg(theme::muted()),
            );
        }
    }
}
