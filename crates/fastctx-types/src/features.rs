//! The optional parts of an interface (`world-gui.md`, 功能) that a device
//! remembers for itself, not for an account, and the language its
//! interfaces speak: a computer's graphical interface and its TUI share one
//! value through its node, and a phone, or a computer used only as a
//! device, keeps its own. Each part is off until the user turns it on, and
//! turning one off hides it without stopping anything it started. No
//! language is chosen until the user chooses one, which the first interface
//! opened asks.

use serde::{Deserialize, Serialize};

/// The languages the interfaces speak, by their tags.
pub const LANGUAGES: [&str; 2] = ["zh", "en"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    /// Talking to agents in the interface: the home page, new
    /// conversations, the projects and conversations in the sidebar, and
    /// the notifications that ask the user something.
    #[serde(default)]
    pub conversations: bool,
    /// The language chosen, by its tag in [`LANGUAGES`]; a tag a later
    /// release speaks reads as none chosen to this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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

    /// The language chosen, when this release speaks it.
    pub fn spoken(&self) -> Option<&str> {
        self.language.as_deref().filter(|tag| LANGUAGES.contains(tag))
    }
}

/// Why `tag` is no language the interfaces speak, when it is none.
pub fn unspoken(tag: &str) -> Option<String> {
    (!LANGUAGES.contains(&tag)).then(|| format!("the interfaces speak {}, not {tag}", LANGUAGES.join(" and ")))
}
