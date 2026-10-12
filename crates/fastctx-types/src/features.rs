//! The optional parts of an interface (`world-gui.md`, 功能) that a device
//! remembers for itself, not for an account: a computer's graphical
//! interface and its TUI share one value through its node, and a phone, or
//! a computer used only as a device, keeps its own. Each is off until the
//! user turns it on, and turning one off hides it without stopping anything
//! it started.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    /// Talking to agents in the interface: the home page, new
    /// conversations, the projects and conversations in the sidebar, and
    /// the notifications that ask the user something.
    #[serde(default)]
    pub conversations: bool,
}

/// One switch of [`Features`], set on its own so that two interfaces
/// changing different switches at once both keep their change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Conversations,
}

impl Features {
    pub fn set(&mut self, feature: Feature, on: bool) {
        match feature {
            Feature::Conversations => self.conversations = on,
        }
    }
}
