//! Terminal color-depth detection and semantic palette degradation.

use ratatui::style::Color;
use std::ffi::OsString;
use std::sync::OnceLock;

/// Color capability available in the terminal; text and symbols always preserve semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ColorMode {
    TrueColor,
    Ansi256,
    Ansi16,
    Monochrome,
}

/// Semantic color roles used by the TUI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Theme {
    pub(crate) bg: Color,
    pub(crate) bg_raised: Color,
    pub(crate) fg: Color,
    pub(crate) accent: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) muted: Color,
    pub(crate) border: Color,
}

impl Theme {
    /// Detect terminal capabilities from the process environment, with NO_COLOR and TERM=dumb taking precedence.
    fn detect() -> Self {
        Self::from_mode(detect_color_mode_with(|name| std::env::var_os(name)))
    }

    /// Map frozen true-color roles to the closest level the terminal can represent.
    pub(crate) const fn from_mode(mode: ColorMode) -> Self {
        match mode {
            ColorMode::TrueColor => Self {
                bg: Color::Rgb(0x04, 0x06, 0x07),
                bg_raised: Color::Rgb(0x13, 0x17, 0x1b),
                fg: Color::Rgb(0xd3, 0xd7, 0xda),
                accent: Color::Rgb(0xf9, 0xf8, 0xf8),
                success: Color::Rgb(0x6e, 0x9e, 0x86),
                warning: Color::Rgb(0xc4, 0x9a, 0x5b),
                danger: Color::Rgb(0xcb, 0x5d, 0x56),
                muted: Color::Rgb(0x73, 0x74, 0x75),
                border: Color::Rgb(0x30, 0x32, 0x36),
            },
            ColorMode::Ansi256 => Self {
                bg: Color::Indexed(232),
                bg_raised: Color::Indexed(234),
                fg: Color::Indexed(252),
                accent: Color::Indexed(15),
                success: Color::Indexed(108),
                warning: Color::Indexed(179),
                danger: Color::Indexed(167),
                muted: Color::Indexed(243),
                border: Color::Indexed(236),
            },
            ColorMode::Ansi16 => Self {
                bg: Color::Black,
                bg_raised: Color::DarkGray,
                fg: Color::Gray,
                accent: Color::White,
                success: Color::Green,
                warning: Color::Yellow,
                danger: Color::Red,
                muted: Color::DarkGray,
                border: Color::DarkGray,
            },
            ColorMode::Monochrome => Self {
                bg: Color::Reset,
                bg_raised: Color::Reset,
                fg: Color::Reset,
                accent: Color::Reset,
                success: Color::Reset,
                warning: Color::Reset,
                danger: Color::Reset,
                muted: Color::Reset,
                border: Color::Reset,
            },
        }
    }
}

fn detect_color_mode_with(mut value: impl FnMut(&str) -> Option<OsString>) -> ColorMode {
    if value("NO_COLOR").is_some() {
        return ColorMode::Monochrome;
    }
    let term = value("TERM")
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if term == "dumb" {
        return ColorMode::Monochrome;
    }
    let color_term = value("COLORTERM")
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if color_term.contains("truecolor")
        || color_term.contains("24bit")
        || term.contains("truecolor")
        || term.contains("24bit")
        || value("WT_SESSION").is_some()
        || value("WEZTERM_EXECUTABLE").is_some()
        || value("TERM_PROGRAM").is_some_and(|program| {
            matches!(
                program.to_string_lossy().to_ascii_lowercase().as_str(),
                "vscode" | "iterm.app" | "wezterm"
            )
        })
    {
        return ColorMode::TrueColor;
    }
    if term.contains("256color") {
        return ColorMode::Ansi256;
    }
    if !term.is_empty() || cfg!(windows) {
        return ColorMode::Ansi16;
    }
    ColorMode::Monochrome
}

static DETECTED_THEME: OnceLock<Theme> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<ColorMode>> = const { std::cell::Cell::new(None) };
}

fn current() -> Theme {
    #[cfg(test)]
    if let Some(mode) = TEST_MODE.get() {
        return Theme::from_mode(mode);
    }
    *DETECTED_THEME.get_or_init(Theme::detect)
}

pub(crate) fn bg() -> Color {
    current().bg
}

pub(crate) fn bg_raised() -> Color {
    current().bg_raised
}

pub(crate) fn fg() -> Color {
    current().fg
}

pub(crate) fn accent() -> Color {
    current().accent
}

pub(crate) fn success() -> Color {
    current().success
}

pub(crate) fn warning() -> Color {
    current().warning
}

pub(crate) fn danger() -> Color {
    current().danger
}

pub(crate) fn muted() -> Color {
    current().muted
}

pub(crate) fn border() -> Color {
    current().border
}

#[cfg(test)]
pub(crate) fn with_test_mode<T>(mode: ColorMode, action: impl FnOnce() -> T) -> T {
    struct Reset(Option<ColorMode>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_MODE.set(self.0);
        }
    }

    let previous = TEST_MODE.replace(Some(mode));
    let _reset = Reset(previous);
    action()
}

#[cfg(test)]
mod tests {
    use super::{ColorMode, Theme, detect_color_mode_with};
    use ratatui::style::Color;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn detect(values: &[(&str, &str)]) -> ColorMode {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        detect_color_mode_with(|name| values.get(name).cloned())
    }

    #[test]
    fn no_color_and_dumb_terminal_force_monochrome() {
        assert_eq!(
            detect(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]),
            ColorMode::Monochrome
        );
        assert_eq!(detect(&[("TERM", "dumb")]), ColorMode::Monochrome);
        assert_eq!(Theme::from_mode(ColorMode::Monochrome).accent, Color::Reset);
    }

    #[test]
    fn terminal_capabilities_degrade_to_256_and_16_color_palettes() {
        assert_eq!(detect(&[("TERM", "xterm-256color")]), ColorMode::Ansi256);
        assert_eq!(detect(&[("TERM", "xterm")]), ColorMode::Ansi16);
        assert!(matches!(
            Theme::from_mode(ColorMode::Ansi256).success,
            Color::Indexed(_)
        ));
        assert_eq!(Theme::from_mode(ColorMode::Ansi16).accent, Color::White);
    }

    #[test]
    fn truecolor_signals_preserve_the_frozen_rgb_palette() {
        assert_eq!(detect(&[("COLORTERM", "truecolor")]), ColorMode::TrueColor);
        assert_eq!(
            Theme::from_mode(ColorMode::TrueColor).bg,
            Color::Rgb(0x04, 0x06, 0x07)
        );
    }
}
