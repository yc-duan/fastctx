//! A managed machine's one screen (`world-gui.md`, 受管机器上的界面): whose
//! machine this computer is, on which network, and whether it is connected;
//! nothing of the World serves anyone sitting at it. The TUI shows it in
//! place of the World while this computer is a managed machine, and goes on
//! to the World once it is one no longer.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_client::World;
use fastctx_types::accounts::AccountsReply;
use fastctx_types::world::LinkState;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::design::{Look, Role};
use crate::input::Input;
use crate::terminal::Screen;
use crate::words::Words;

/// How often whether it is connected is read again.
const EVERY: Duration = Duration::from_secs(3);

/// This computer as a managed machine: whose, its name, on which network,
/// and how it is connected.
pub struct Managed {
    owner: String,
    machine: String,
    hub: String,
    link: LinkState,
}

impl Managed {
    /// This computer as a managed machine, when it is one.
    pub fn of(accounts: &AccountsReply) -> Option<Self> {
        accounts.hubs.iter().find_map(|hub| {
            let here = hub.me.as_ref().unwrap_or(&accounts.machine);
            let me = hub.members.iter().find(|member| member.name == *here || member.was.contains(here))?;
            me.managed.then(|| Managed {
                owner: me.home.as_ref().map(ToString::to_string).unwrap_or_default(),
                machine: me.name.to_string(),
                hub: hub.name.to_string(),
                link: hub.link.clone(),
            })
        })
    }

    fn draw(&self, frame: &mut Frame, look: &Look, words: &Words) {
        let area = frame.area();
        frame.render_widget(Block::default().style(look.ground()), area);
        let now = Timestamp::now();
        let (mark, state, role) = match &self.link {
            LinkState::Online { ms, .. } => ("●", words.managed_online(*ms), Role::Alive),
            LinkState::Connecting { since, .. } => {
                ("○", words.managed_away(&crate::words::clock(*since, now)), Role::Caution)
            }
            LinkState::Stopped { .. } => ("○", words.managed_removed().to_string(), Role::Wrong),
            LinkState::Alone => ("○", String::new(), Role::Faint),
        };
        let lines = vec![
            Line::styled(words.managed_title(), look.text(Role::Text).add_modifier(ratatui::style::Modifier::BOLD)),
            Line::raw(""),
            Line::styled(words.managed_whose(&self.owner, &self.machine, &self.hub), look.text(Role::Muted)),
            Line::raw(""),
            Line::from(vec![
                Span::styled(format!("{mark} "), look.text(role)),
                Span::styled(state, look.text(Role::Muted)),
            ]),
        ];
        let height = lines.len() as u16;
        let top = area.y + area.height.saturating_sub(height) / 2;
        let body = Rect { x: area.x, y: top, width: area.width, height: height.min(area.height) };
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center).style(look.ground()), body);
        if area.height > height + 2 {
            let foot = Rect { x: area.x, y: area.y + area.height - 1, width: area.width, height: 1 };
            let hint = Line::from(vec![
                Span::styled("q ", look.text(Role::Accent)),
                Span::styled(words.key_quit(), look.text(Role::Faint)),
            ]);
            frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center).style(look.ground()), foot);
        }
    }
}

/// Shows the screen until the user leaves it, `true` then, or until this
/// computer is a managed machine no longer, `false`.
pub async fn show(
    world: &World,
    screen: &mut Screen,
    inputs: &mut UnboundedReceiver<Input>,
    look: Look,
    words: &Words,
    mut managed: Managed,
) -> anyhow::Result<bool> {
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        screen.terminal.draw(|frame| managed.draw(frame, &look, words))?;
        tokio::select! {
            input = inputs.recv() => match input {
                Some(Input::Key(key)) if leaves(&key) => return Ok(true),
                Some(Input::Failed(why)) => anyhow::bail!("the terminal's input could not be read: {why}"),
                None => return Ok(true),
                Some(_) => {}
            },
            _ = every.tick() => {
                // A node that does not answer for a moment leaves the screen
                // as it was.
                if let Ok(accounts) = world.accounts().await {
                    match Managed::of(&accounts) {
                        Some(now) => managed = now,
                        None => return Ok(false),
                    }
                }
            }
        }
    }
}

/// q, Esc and Ctrl+C leave.
fn leaves(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}
