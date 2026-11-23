//! Model collaboration belongs to an account, not to a project or session;
//! a computer in no network keeps its own (`world-colleagues.md`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::collab_presets::{Preset, Texts};
use crate::{AgentName, MachineName};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLogin {
    pub machine: MachineName,
    pub agent: AgentName,
    pub home: Option<PathBuf>,
}

/// A local tool adapter's agent and login; its node supplies the machine identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub agent: AgentName,
    pub home: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callee {
    pub name: String,
    pub use_when: String,
    pub login: AgentLogin,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default = "screen_default")]
    pub screen: bool,
    /// How many of its colleagues may be at work at once; none says the
    /// default, as settings from before it was set do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_once: Option<AtOnce>,
    /// What it is asked for; none as a client from before presets wrote it,
    /// which is the World's reviewer with the account's text for callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
}

impl Callee {
    /// Which of the presets it is.
    pub fn kind(&self) -> crate::collab_presets::Kind {
        self.preset.as_ref().map(|preset| preset.kind).unwrap_or_default()
    }

    /// What the account adds to its rules.
    pub fn rules(&self) -> &str {
        self.preset.as_ref().map_or("", |preset| &preset.rules)
    }

    /// Whether a machine from before presets would start it as it is meant:
    /// such a machine starts every callee as the reviewer, in a Codex login.
    pub fn plain(&self) -> bool {
        self.login.agent.as_str() == "codex" && self.preset.as_ref().is_none_or(Preset::plain)
    }
}

fn screen_default() -> bool {
    true
}

/// How many colleagues may be at work at once for one conversation, of one
/// callee or of them all: past that a colleague's turn waits for one to end
/// (`world-colleagues.md`, 同事从哪里来、能去哪里).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtOnce {
    Most(std::num::NonZeroU32),
    Any,
}

impl AtOnce {
    /// What a callee whose settings say nothing works with.
    pub const DEFAULT: AtOnce = AtOnce::Most(std::num::NonZeroU32::new(4).expect("four"));

    /// What a conversation's colleagues of every callee together work
    /// within, where the settings say nothing.
    pub const IN_ALL_DEFAULT: AtOnce = AtOnce::Most(std::num::NonZeroU32::new(8).expect("eight"));

    /// The most at once, or none for no limit.
    pub fn most(this: Option<AtOnce>) -> Option<u32> {
        Self::said(this.unwrap_or(Self::DEFAULT))
    }

    /// The most of every callee together at once, or none for no limit.
    pub fn most_in_all(this: Option<AtOnce>) -> Option<u32> {
        Self::said(this.unwrap_or(Self::IN_ALL_DEFAULT))
    }

    fn said(self) -> Option<u32> {
        match self {
            AtOnce::Most(most) => Some(most.get()),
            AtOnce::Any => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collab {
    pub version: u64,
    pub on: bool,
    pub callers: Vec<AgentLogin>,
    /// The presets turned on: what callers are offered, by every release.
    pub callees: Vec<Callee>,
    /// The account's text for callers of a callee written before presets;
    /// one written since carries its own.
    pub prompt: Option<String>,
    /// Given to every callee after its own rules.
    pub rules: String,
    /// The presets kept while turned off, which no caller is offered; none
    /// as a client from before presets writes the settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelved: Option<Vec<Callee>>,
    /// How many colleagues one conversation may have at work at once, of
    /// every callee together; none says the default, as settings from
    /// before it was set do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_once: Option<AtOnce>,
}

impl Collab {
    /// Every preset the settings keep: those turned on, then those off.
    pub fn kept(&self) -> impl Iterator<Item = &Callee> {
        self.callees.iter().chain(self.shelved.iter().flatten())
    }

    /// The text given to the callers of `callee`, one of these settings'.
    pub fn text_for<'a>(&'a self, callee: &'a Callee) -> &'a str {
        match &callee.preset {
            Some(preset) => preset.prompt(),
            None => self.prompt.as_deref().unwrap_or(crate::collab_prompt::DEFAULT_PROMPT),
        }
    }

    /// Keeps of `current` what these settings leave unsaid: a client from
    /// before a callee's limit, or before presets, knows no such field and
    /// would write it away. A preset kept while off stays so unless these
    /// settings turn one of its name on.
    pub fn keep_unsaid(&mut self, current: Option<&Collab>) {
        let Some(current) = current else { return };
        for callee in self.callees.iter_mut().chain(self.shelved.iter_mut().flatten()) {
            let kept = current.kept().find(|kept| kept.name == callee.name);
            if callee.at_once.is_none() {
                callee.at_once = kept.and_then(|kept| kept.at_once);
            }
            if callee.preset.is_none() {
                callee.preset = kept.and_then(|kept| kept.preset.clone());
            }
        }
        if self.at_once.is_none() {
            self.at_once = current.at_once;
        }
        if self.shelved.is_none() && current.shelved.is_some() {
            let on = |kept: &&Callee| self.callees.iter().all(|callee| callee.name != kept.name);
            self.shelved = Some(current.shelved.iter().flatten().filter(on).cloned().collect());
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        for callee in self.kept() {
            if let Some(preset) = &callee.preset {
                preset.validate(&callee.name)?;
            }
            if callee.name.is_empty()
                || !callee.name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            {
                return Err(format!("callee name {:?} must contain lowercase letters, digits or hyphens", callee.name));
            }
            if !names.insert(&callee.name) {
                return Err(format!("callee name {} is repeated in this account", callee.name));
            }
            if !matches!(callee.login.agent.as_str(), "codex" | "claude") {
                return Err(format!("{} must use a Codex or a Claude Code login", callee.name));
            }
        }
        for caller in &self.callers {
            if !matches!(caller.agent.as_str(), "codex" | "claude") {
                return Err(format!("{} is not a Codex or Claude login", caller.agent));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollabView {
    pub collab: Collab,
    /// The World's text for the callers of its reviewer.
    pub default_prompt: String,
    /// The World's presets and their texts; none from a release before them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Texts>,
    /// Whether where these settings are kept keeps presets: a hub from
    /// before them would write a preset's kind and texts, and those kept
    /// while off, away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keeps_presets: bool,
}

impl CollabView {
    pub fn new(collab: Collab, keeps_presets: bool) -> Self {
        CollabView {
            collab,
            default_prompt: crate::collab_prompt::DEFAULT_PROMPT.to_string(),
            presets: crate::collab_presets::texts(),
            keeps_presets,
        }
    }
}

/// The caller's tool projection; account configuration remains on the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offered {
    pub callees: Vec<Offer>,
    pub prompt: String,
    /// How many of the caller's colleagues of every callee together may be
    /// at work at once; none for no limit, as a node from before it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_once: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub name: String,
    /// The account it works for; none for a callee of a computer in no
    /// network, which works for that computer's user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub use_when: String,
    /// The agent that runs it, Codex or Claude Code; none from a node before
    /// a callee could be Claude Code's, whose callees are all Codex's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub lives_on: Option<String>,
    /// How many of its colleagues may be at work at once for the caller;
    /// none for no limit, as a machine from before the limit says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_once: Option<u32>,
}

/// The goal runtime's summary for a colleague shown in the collaboration tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSummary {
    pub state: GoalState,
    pub started_at: jiff::Timestamp,
    pub elapsed_ms: u64,
    /// The goal itself, and the asker's goal it branched from, for the
    /// drawing of the conversation's goals (`world-goal.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    Active,
    Complete,
    Blocked,
    Paused,
}

/// Kept with a callee so cold resumes retain the configuration it was born with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub callee: Callee,
    pub rules: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab_presets::Kind;

    fn callee(name: &str, preset: Option<Preset>) -> Callee {
        Callee {
            name: name.into(),
            use_when: String::new(),
            login: AgentLogin { machine: "here".parse().unwrap(), agent: "codex".parse().unwrap(), home: None },
            model: None,
            effort: None,
            screen: true,
            at_once: None,
            preset,
        }
    }

    fn scout() -> Option<Preset> {
        Some(Preset { kind: Kind::Scout, prompt: None, rules: "stay off prod".into() })
    }

    /// A client from before presets reads the callees turned on and writes
    /// them back as it knows them: what it cannot say stays as it was.
    #[test]
    fn a_write_from_before_presets_keeps_each_preset_and_those_kept_off() {
        let current = Collab {
            version: 4,
            on: true,
            callees: vec![callee("gpt", Some(Preset::default())), callee("scout", scout())],
            shelved: Some(vec![callee("mine", Some(Preset { kind: Kind::Own, prompt: Some("p".into()), rules: "r".into() }))]),
            ..Default::default()
        };
        let mut written = Collab {
            version: 4,
            on: true,
            callees: vec![callee("scout", None), callee("new", None)],
            ..Default::default()
        };
        written.keep_unsaid(Some(&current));
        assert_eq!(written.callees[0].preset, scout());
        assert_eq!(written.callees[1].preset, None);
        assert_eq!(written.shelved, current.shelved);

        // It turns on a callee of the name of one kept off: that one is on now.
        let mut written = Collab { callees: vec![callee("mine", None)], ..Default::default() };
        written.keep_unsaid(Some(&current));
        assert_eq!(written.callees[0].preset.as_ref().map(|preset| preset.kind), Some(Kind::Own));
        assert_eq!(written.shelved, Some(Vec::new()));
        assert!(written.validate().is_ok());
    }

    #[test]
    fn a_client_that_knows_presets_says_all_of_them() {
        let current = Collab {
            callees: vec![callee("gpt", Some(Preset::default()))],
            shelved: Some(vec![callee("scout", scout())]),
            ..Default::default()
        };
        // Turned on: said among the callees, and no longer among those off.
        let mut written =
            Collab { callees: vec![callee("gpt", Some(Preset::default())), callee("scout", scout())], shelved: Some(Vec::new()), ..Default::default() };
        written.keep_unsaid(Some(&current));
        assert_eq!(written.shelved, Some(Vec::new()));
        assert_eq!(written.callees.len(), 2);
    }

    #[test]
    fn a_name_is_one_presets_whether_on_or_off() {
        let collab = Collab {
            callees: vec![callee("gpt", None)],
            shelved: Some(vec![callee("gpt", scout())]),
            ..Default::default()
        };
        assert!(collab.validate().unwrap_err().contains("repeated"));
        let long = Preset { kind: Kind::Own, prompt: Some("x".repeat(Preset::TEXT_MOST + 1)), rules: String::new() };
        assert!(Collab { callees: vec![callee("gpt", Some(long))], ..Default::default() }.validate().is_err());
    }

    #[test]
    fn a_callee_from_before_presets_is_given_the_accounts_text() {
        let collab = Collab { prompt: Some("the account's".into()), ..Default::default() };
        assert_eq!(collab.text_for(&callee("gpt", None)), "the account's");
        assert_eq!(collab.text_for(&callee("gpt", Some(Preset::default()))), crate::collab_prompt::DEFAULT_PROMPT);
        assert_eq!(collab.text_for(&callee("scout", scout())), Kind::Scout.prompt());
        let none = Collab::default();
        assert_eq!(none.text_for(&callee("gpt", None)), crate::collab_prompt::DEFAULT_PROMPT);
        assert!(callee("gpt", None).plain() && !callee("scout", scout()).plain());
        let mut claude = callee("gpt", None);
        claude.login.agent = "claude".parse().unwrap();
        assert!(!claude.plain());
        assert!(Collab { callees: vec![claude], ..Default::default() }.validate().is_ok());
        let wire = serde_json::to_value(callee("gpt", None)).unwrap();
        assert!(wire.get("preset").is_none() && serde_json::to_value(&none).unwrap().get("shelved").is_none());
    }
}
