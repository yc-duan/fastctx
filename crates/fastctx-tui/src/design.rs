//! The World's design language as the terminal speaks it (`world-gui.md`,
//! 审美): literary rather than a cold console. Its three themes are the
//! user's own, value for value: warm black, a warm near-black
//! ground with greys in steps of brightness and a terracotta accent shown
//! sparingly; ink black, its colourless sibling, whose accent is a grey and
//! whose user's words sit on an inverted, near-white ground; and warm
//! white, the light one. The graphical interface takes the same tokens.
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
    /// 暖黑: the dark theme by default.
    WarmBlack,
    /// 墨黑: the dark theme without hue.
    InkBlack,
    /// 暖白: the light theme.
    WarmWhite,
}

/// A role a colour plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The ground everything sits on.
    Ground,
    /// A shade off the ground: the selected row, the place words are typed.
    Raised,
    /// The ground of the words that went in to an agent.
    Bubble,
    /// Text on that ground.
    OnBubble,
    /// The ground of code.
    Code,
    /// A hairline between the parts of a page.
    Rule,
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
/// interface takes too. All but `Caution` are the themes' own (`--ink-surface-*`,
/// `--ink-text-*`, `--ink-accent*`, `--ink-bubble-*`, the diff colours for
/// alive and wrong; the rule is `--ink-border-strong` laid over the
/// ground); the themes have no colour for something to look at.
pub const fn token(theme: Theme, role: Role) -> (u8, u8, u8) {
    match (theme, role) {
        (Theme::WarmBlack, Role::Ground) => (0x1f, 0x1f, 0x1e),
        (Theme::WarmBlack, Role::Raised) => (0x2c, 0x2c, 0x2a),
        (Theme::WarmBlack, Role::Bubble) => (0x12, 0x12, 0x12),
        (Theme::WarmBlack, Role::OnBubble) => (0xf5, 0xf2, 0xe8),
        (Theme::WarmBlack, Role::Code) => (0x27, 0x27, 0x27),
        (Theme::WarmBlack, Role::Rule) => (0x41, 0x41, 0x3f),
        (Theme::WarmBlack, Role::Text) => (0xf8, 0xf8, 0xf6),
        (Theme::WarmBlack, Role::Muted) => (0xbc, 0xbc, 0xb9),
        (Theme::WarmBlack, Role::Faint) => (0x98, 0x98, 0x98),
        (Theme::WarmBlack, Role::Accent) => (0xd9, 0x77, 0x57),
        (Theme::WarmBlack, Role::Alive) => (0x7c, 0xb2, 0x7c),
        (Theme::WarmBlack, Role::Caution) => (0xd4, 0xa8, 0x5a),
        (Theme::WarmBlack, Role::Wrong) => (0xc9, 0x6a, 0x6a),
        (Theme::WarmBlack, Role::Link) => (0xe0, 0xa1, 0x8b),
        (Theme::InkBlack, Role::Ground) => (0x0a, 0x0b, 0x0b),
        (Theme::InkBlack, Role::Raised) => (0x21, 0x21, 0x21),
        (Theme::InkBlack, Role::Bubble) => (0xfb, 0xfb, 0xfc),
        (Theme::InkBlack, Role::OnBubble) => (0x0a, 0x0b, 0x0b),
        (Theme::InkBlack, Role::Code) => (0x1a, 0x1b, 0x1c),
        (Theme::InkBlack, Role::Rule) => (0x31, 0x32, 0x32),
        (Theme::InkBlack, Role::Text) => (0xfb, 0xfb, 0xfc),
        (Theme::InkBlack, Role::Muted) => (0xaa, 0xaa, 0xab),
        (Theme::InkBlack, Role::Faint) => (0x83, 0x84, 0x84),
        (Theme::InkBlack, Role::Accent) => (0xc2, 0xc4, 0xc7),
        (Theme::InkBlack, Role::Alive) => (0x7c, 0xb2, 0x7c),
        (Theme::InkBlack, Role::Caution) => (0xd4, 0xa8, 0x5a),
        (Theme::InkBlack, Role::Wrong) => (0xc9, 0x6a, 0x6a),
        (Theme::InkBlack, Role::Link) => (0xda, 0xdc, 0xe0),
        (Theme::WarmWhite, Role::Ground) => (0xf8, 0xf8, 0xf6),
        (Theme::WarmWhite, Role::Raised) => (0xed, 0xeb, 0xe5),
        (Theme::WarmWhite, Role::Bubble) => (0xe6, 0xe4, 0xde),
        (Theme::WarmWhite, Role::OnBubble) => (0x0d, 0x0d, 0x0d),
        (Theme::WarmWhite, Role::Code) => (0xe6, 0xe4, 0xde),
        (Theme::WarmWhite, Role::Rule) => (0xda, 0xd9, 0xd5),
        (Theme::WarmWhite, Role::Text) => (0x0d, 0x0d, 0x0d),
        (Theme::WarmWhite, Role::Muted) => (0x79, 0x75, 0x69),
        (Theme::WarmWhite, Role::Faint) => (0x9a, 0x96, 0x8b),
        (Theme::WarmWhite, Role::Accent) => (0xbf, 0x5d, 0x3a),
        (Theme::WarmWhite, Role::Alive) => (0x1c, 0x9c, 0x1c),
        (Theme::WarmWhite, Role::Caution) => (0x9a, 0x6d, 0x1a),
        (Theme::WarmWhite, Role::Wrong) => (0xc9, 0x26, 0x26),
        (Theme::WarmWhite, Role::Link) => (0x8f, 0x45, 0x2b),
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
                Role::Ground | Role::Raised | Role::Bubble | Role::OnBubble | Role::Code | Role::Text => None,
                Role::Muted | Role::Faint => Some(Color::Gray),
                Role::Rule => Some(Color::DarkGray),
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
            (Depth::Mono, Role::Rule) => style.add_modifier(Modifier::DIM),
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

    /// The words that went in to an agent, on their own ground.
    pub fn bubble(&self) -> Style {
        let style = Style::default();
        match (self.color(Role::Bubble), self.color(Role::OnBubble)) {
            (Some(ground), Some(text)) => style.bg(ground).fg(text),
            _ => self.text(Role::Text),
        }
    }

    /// Code in a role, on the ground of code.
    pub fn code(&self, role: Role) -> Style {
        let style = self.text(role);
        match self.color(Role::Code) {
            Some(code) => style.bg(code),
            None => style,
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

/// `FASTCTX_THEME` when set: `warm-black` (or `dark`), `ink-black`, or
/// `warm-white` (or `light`). Otherwise the native Windows application
/// preference picks warm white or warm black. On other platforms, or if that
/// preference is unavailable, `COLORFGBG` is taken as a palette hint, and
/// finally warm black. A palette hint is not a system preference: terminal
/// palettes can be customized, and SSH does not carry an OS theme.
pub fn theme() -> Theme {
    match std::env::var("FASTCTX_THEME").unwrap_or_default().to_lowercase().as_str() {
        "warm-black" | "dark" => return Theme::WarmBlack,
        "ink-black" => return Theme::InkBlack,
        "warm-white" | "light" => return Theme::WarmWhite,
        _ => {}
    }
    if let Some(light) = system_light() {
        return if light { Theme::WarmWhite } else { Theme::WarmBlack };
    }
    // `COLORFGBG` is `fg;bg` in the 16 colours; a light ground is 7 or 15.
    match std::env::var("COLORFGBG").ok().and_then(|value| value.rsplit(';').next().map(str::to_string)).as_deref() {
        Some("7" | "15") => Theme::WarmWhite,
        _ => Theme::WarmBlack,
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
