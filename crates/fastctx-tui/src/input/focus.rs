//! Focus reported by the terminal, shared with the screen without waiting for
//! the UI event queue to catch up. A newly opened reader knows no focus state.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Focus {
    #[default]
    Unknown,
    Focused,
    Unfocused,
}

static FOCUS: AtomicU8 = AtomicU8::new(Focus::Unknown as u8);

/// The latest decoded focus report. Enabling reporting does not establish the
/// initial focus, and unsupported terminals leave this as `Unknown`.
pub fn focus() -> Focus {
    match FOCUS.load(Ordering::Acquire) {
        1 => Focus::Focused,
        2 => Focus::Unfocused,
        _ => Focus::Unknown,
    }
}

pub(super) fn set(focus: Focus) {
    FOCUS.store(focus as u8, Ordering::Release);
}
