//! The device notification policy, shared by asset validation and platform adapters.
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Policy {
    pub enabled: bool,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub agent_names: BTreeMap<String, String>,
    #[serde(default)]
    pub troubles: Option<TroubleWords>,
}

/// What went wrong, in the page's words: a title and a detail for each kind
/// of trouble, `{machine}`, `{count}`, `{seconds}`, `{bound}`, `{used}` and
/// `{version}` standing for what the trouble says, and the bounds by their
/// plain names (`gui/src/app/phone.ts`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TroubleWords {
    pub titles: BTreeMap<String, String>,
    pub details: BTreeMap<String, String>,
    pub bounds: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Rule {
    pub when: Transition,
    pub user_started_only: bool,
    pub title: Title,
    /// The body of a turn that compacted the conversation and said nothing
    /// else, in the page's words.
    #[serde(default)]
    pub compacted: Option<String>,
}

/// A title, or one for each kind: each thing an agent may ask (`AskedFor`,
/// and `waiting` for a host that only says it waits), or each way it tells
/// the user something worth a call. A kind with no title is not called about.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Title {
    Text(String),
    Kinds(BTreeMap<String, String>),
}

impl Title {
    pub fn template(&self, kind: Option<&str>) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Kinds(kinds) => kinds.get(kind?).map(String::as_str),
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    Asked,
    Told,
    Finished,
}

pub fn validate(json: &str) -> Result<(), String> {
    let policy: Policy = serde_json::from_str(json).map_err(|error| error.to_string())?;
    for (index, rule) in policy.rules.iter().enumerate() {
        let untitled = match &rule.title {
            Title::Text(text) => text.is_empty(),
            Title::Kinds(kinds) => kinds.is_empty() || kinds.values().any(String::is_empty),
        };
        if untitled {
            return Err("a notification rule needs a title".into());
        }
        if policy.rules[..index].iter().any(|other| other.when == rule.when) {
            return Err("only one rule for each transition may be installed".into());
        }
    }
    Ok(())
}
