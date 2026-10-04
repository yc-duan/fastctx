//! The World's design language as the terminal speaks it (`world-gui.md`,
//! 审美): soft and literary, warm paper and ink rather than a cold console.
//! The graphical interface takes the same tokens.
//!
//! Colour is a role, never a value written where it is used. Each role has
//! a value for each of the two themes, ink (dark) and paper (light), in
//! true colour; the 256-colour palette takes the nearest of its own; with
//! 16 colours the terminal's own background and a fixed choice of its named
//! colours stand in; with none, only weight, dimness and reversal remain.
//!
//! Hierarchy comes from weight, brightness and space, not from lines: no
//! boxes, a gutter of two cells, a blank line between sections, the
//! selected row a shade lighter with a mark in the gutter. What moves moves
//! only because something is happening, and quietly.

use ratatui::style::{Color, Modifier, Style};

/// How many colours the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Warm ink: a dark ground.
    Ink,
    /// Warm paper: a light ground.
    Paper,
}

/// A role a colour plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The ground everything sits on.
    Ground,
    /// A shade off the ground: the selected row, the place words are typed.
    Raised,
    Text,
    /// What is said about a thing rather than the thing.
    Muted,
    /// What is there to be found, not read: hints, separators, the far past.
    Faint,
    /// Where the eye goes: what is selected, what a key does.
    Accent,
    /// Something happening now: a resident at work, a machine online.
    Alive,
    /// Something to look at: a session in trouble, a machine that did not
    /// answer.
    Caution,
    /// Something that went wrong.
    Wrong,
    /// Something to follow: a path, a machine's name in passing.
    Link,
}

/// The values of each role, as `#rrggbb`: the tokens the graphical
/// interface takes too.
pub const fn token(theme: Theme, role: Role) -> (u8, u8, u8) {
    match (theme, role) {
        (Theme::Ink, Role::Ground) => (0x1d, 0x1b, 0x18),
        (Theme::Ink, Role::Raised) => (0x29, 0x26, 0x21),
        (Theme::Ink, Role::Text) => (0xec, 0xe5, 0xd8),
        (Theme::Ink, Role::Muted) => (0xa8, 0x9f, 0x8f),
        (Theme::Ink, Role::Faint) => (0x6e, 0x66, 0x5a),
        (Theme::Ink, Role::Accent) => (0xe0, 0x9b, 0x6b),
        (Theme::Ink, Role::Alive) => (0x9c, 0xc0, 0x8d),
        (Theme::Ink, Role::Caution) => (0xdd, 0xbb, 0x72),
        (Theme::Ink, Role::Wrong) => (0xdc, 0x83, 0x77),
        (Theme::Ink, Role::Link) => (0x93, 0xae, 0xc9),
        (Theme::Paper, Role::Ground) => (0xf6, 0xf1, 0xe7),
        (Theme::Paper, Role::Raised) => (0xeb, 0xe4, 0xd6),
        (Theme::Paper, Role::Text) => (0x2c, 0x28, 0x23),
        (Theme::Paper, Role::Muted) => (0x6e, 0x66, 0x5a),
        (Theme::Paper, Role::Faint) => (0xa6, 0x9d, 0x8d),
        (Theme::Paper, Role::Accent) => (0xb5, 0x60, 0x34),
        (Theme::Paper, Role::Alive) => (0x4f, 0x7d, 0x47),
        (Theme::Paper, Role::Caution) => (0x9a, 0x74, 0x1f),
        (Theme::Paper, Role::Wrong) => (0xb0, 0x45, 0x3b),
        (Theme::Paper, Role::Link) => (0x3f, 0x63, 0x8c),
    }
}

/// The look a screen is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub depth: Depth,
    pub theme: Theme,
}

impl Look {
    /// The colour of a role at this depth; `None` where the terminal's own
    /// stands in.
    pub fn color(&self, role: Role) -> Option<Color> {
        match self.depth {
            Depth::TrueColor => {
                let (r, g, b) = token(self.theme, role);
                Some(Color::Rgb(r, g, b))
            }
            Depth::Ansi256 => Some(Color::Indexed(nearest_256(token(self.theme, role)))),
            Depth::Ansi16 => match role {
                // The terminal's own ground and text, whatever they are.
                Role::Ground | Role::Raised | Role::Text => None,
                Role::Muted | Role::Faint => Some(Color::Gray),
                Role::Accent => Some(Color::Yellow),
                Role::Alive => Some(Color::Green),
                Role::Caution => Some(Color::Yellow),
                Role::Wrong => Some(Color::Red),
                Role::Link => Some(Color::Cyan),
            },
            Depth::Mono => None,
        }
    }

    /// Text in a role, on the ground.
    pub fn text(&self, role: Role) -> Style {
        let mut style = Style::default();
        if let Some(color) = self.color(role) {
            style = style.fg(color);
        }
        if let Some(ground) = self.color(Role::Ground) {
            style = style.bg(ground);
        }
        match (self.depth, role) {
            // Without colour, what is said about a thing steps back.
            (Depth::Ansi16 | Depth::Mono, Role::Faint) => style.add_modifier(Modifier::DIM),
            (Depth::Mono, Role::Muted) => style.add_modifier(Modifier::DIM),
            (Depth::Mono, Role::Accent | Role::Wrong) => style.add_modifier(Modifier::BOLD),
            _ => style,
        }
    }

    /// Text in a role on the raised ground: the selected row.
    pub fn raised(&self, role: Role) -> Style {
        let style = self.text(role);
        match self.color(Role::Raised) {
            Some(raised) => style.bg(raised),
            // Without a colour to raise it, the row is reversed.
            None if role == Role::Text => style.add_modifier(Modifier::REVERSED),
            None => style,
        }
    }

    /// Text in a role drawn toward the ground by `amount`, from 0 (as it
    /// is) to 1 (gone): how a mark of something happening breathes.
    pub fn faded(&self, role: Role, amount: f32) -> Style {
        let amount = amount.clamp(0.0, 1.0);
        let style = self.text(role);
        let mixed = || {
            let (from, to) = (token(self.theme, role), token(self.theme, Role::Ground));
            let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
            (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
        };
        match self.depth {
            Depth::TrueColor => {
                let (r, g, b) = mixed();
                style.fg(Color::Rgb(r, g, b))
            }
            Depth::Ansi256 => style.fg(Color::Indexed(nearest_256(mixed()))),
            Depth::Ansi16 | Depth::Mono if amount >= 0.5 => style.add_modifier(Modifier::DIM),
            Depth::Ansi16 | Depth::Mono => style,
        }
    }

    /// The ground, for clearing a screen.
    pub fn ground(&self) -> Style {
        match self.color(Role::Ground) {
            Some(ground) => Style::default().bg(ground).fg(self.color(Role::Text).unwrap_or(Color::Reset)),
            None => Style::default(),
        }
    }
}

/// The colour depth advertised to this process, respecting `NO_COLOR`.
///
/// A pipe or file gets no colours. This is separate from permission to
/// start a full-screen interface: call [`crate::terminal::require_interactive`]
/// before changing terminal modes. Unknown capabilities and failed platform
/// queries are errors, rather than guesses about a terminal's palette.
/// Detect during startup, before other threads change console modes.
pub fn depth() -> std::io::Result<Depth> {
    crate::terminal::depth()
}

/// Ink or paper: `FASTCTX_THEME` when set (`ink`, `paper`, `dark` or
/// `light`), otherwise the native Windows application preference. On other
/// platforms, or if that preference is unavailable, use `COLORFGBG` as a
/// palette hint and finally ink. A palette hint is not a system preference:
/// terminal palettes can be customized, and SSH does not carry an OS theme.
pub fn theme() -> Theme {
    match std::env::var("FASTCTX_THEME").unwrap_or_default().to_lowercase().as_str() {
        "paper" | "light" => return Theme::Paper,
        "ink" | "dark" => return Theme::Ink,
        _ => {}
    }
    if let Some(light) = system_light() {
        return if light { Theme::Paper } else { Theme::Ink };
    }
    // `COLORFGBG` is `fg;bg` in the 16 colours; a light ground is 7 or 15.
    match std::env::var("COLORFGBG").ok().and_then(|value| value.rsplit(';').next().map(str::to_string)).as_deref() {
        Some("7" | "15") => Theme::Paper,
        _ => Theme::Ink,
    }
}

/// Whether the system asks applications for light colours.
#[cfg(windows)]
fn system_light() -> Option<bool> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: the names are NUL-terminated and the data buffer is a u32
    // whose size is passed with it.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    match (status, data) {
        (0, 0) => Some(false),
        (0, 1) => Some(true),
        _ => None,
    }
}

#[cfg(not(windows))]
fn system_light() -> Option<bool> {
    None
}

/// The nearest of the 256-colour palette: its 6×6×6 cube or its 24 greys.
fn nearest_256((r, g, b): (u8, u8, u8)) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |value: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| (i32::from(**level) - i32::from(value)).abs())
            .map(|(index, _)| index as u8)
            .expect("six levels")
    };
    let (cr, cg, cb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * cr + 6 * cg + cb;
    let cube_rgb = (LEVELS[cr as usize], LEVELS[cg as usize], LEVELS[cb as usize]);
    let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey_index = ((average.saturating_sub(8)) / 10).min(23) as u8;
    let grey = 8 + 10 * grey_index;
    let distance = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if distance((grey, grey, grey)) < distance(cube_rgb) { 232 + grey_index } else { cube }
}
