//! The first screen of a computer whose interfaces have no language yet
//! (`world-gui.md`, 第一次打开): the languages they speak, each in its own
//! words, the system's first. The one chosen is kept by the node, so the
//! graphical interface speaks it too.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::design::{Look, Role};
use crate::input::Input;
use crate::terminal::Screen;
use crate::words::Lang;

/// Each language, said in itself.
const NAMES: [(Lang, &str); 2] = [(Lang::Zh, "中文"), (Lang::En, "English")];

/// Asks which language the interfaces speak, the system's offered first;
/// none when the user leaves instead.
pub async fn choose(
    screen: &mut Screen,
    inputs: &mut UnboundedReceiver<Input>,
    look: Look,
    first: Lang,
) -> anyhow::Result<Option<Lang>> {
    let order = ordered(first);
    let mut at = 0usize;
    loop {
        screen.terminal.draw(|frame| draw(frame, &look, first, at))?;
        match inputs.recv().await {
            Some(Input::Key(key)) => match key.code {
                KeyCode::Up | KeyCode::Left | KeyCode::BackTab => at = at.saturating_sub(1),
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => at = (at + 1).min(order.len() - 1),
                KeyCode::Enter | KeyCode::Char(' ') => return Ok(Some(order[at].0)),
                _ if leaves(&key) => return Ok(None),
                _ => {}
            },
            Some(Input::Failed(why)) => anyhow::bail!("the terminal's input could not be read: {why}"),
            None => return Ok(None),
            Some(_) => {}
        }
    }
}

/// The languages, `first` first.
fn ordered(first: Lang) -> Vec<(Lang, &'static str)> {
    let mut order = NAMES.to_vec();
    order.sort_by_key(|(lang, _)| *lang != first);
    order
}

/// The screen, `first` offered first and the one at `at` chosen; drawn on
/// its own for the previews too.
pub fn draw(frame: &mut Frame, look: &Look, first: Lang, at: usize) {
    let order = ordered(first);
    let area = frame.area();
    frame.render_widget(Block::default().style(look.ground()), area);
    // Asked in both, as nothing tells yet which the reader reads.
    let mut lines = vec![Line::styled("选择语言 · Choose a language", look.text(Role::Muted)), Line::raw("")];
    // One column of the same width, so the names start under each other.
    let widest = order.iter().map(|(_, name)| crate::text::width(name)).max().unwrap_or(0);
    for (index, (_, name)) in order.iter().enumerate() {
        let chosen = index == at;
        let style = if chosen { look.raised(Role::Text) } else { look.text(Role::Text) };
        let mark = if chosen { Span::styled(" › ", look.raised(Role::Accent)) } else { Span::styled("   ", style) };
        let room = " ".repeat(widest - crate::text::width(name) + 2);
        lines.push(Line::from(vec![mark, Span::styled(format!("{name}{room}"), style)]));
    }
    let height = lines.len() as u16;
    let top = area.y + area.height.saturating_sub(height) / 2;
    let body = Rect { x: area.x, y: top, width: area.width, height: height.min(area.height) };
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center).style(look.ground()), body);
    if area.height > height + 2 {
        let foot = Rect { x: area.x, y: area.y + area.height - 1, width: area.width, height: 1 };
        let hint = Line::from(vec![
            Span::styled("↑↓ ", look.text(Role::Accent)),
            Span::styled("· ", look.text(Role::Faint)),
            Span::styled("Enter", look.text(Role::Accent)),
        ]);
        frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center).style(look.ground()), foot);
    }
}

/// q, Esc and Ctrl+C leave.
fn leaves(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}
