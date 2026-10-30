//! The account's skills (`world-user-config.md`, 技能): instructions the
//! user keeps under a name, each with a sentence on when it applies, for the
//! agents that work for the account. They are the account's alone: kept in
//! its records at its hub, one record a skill and one index of their names
//! and sentences, written only by the user. A session working for one
//! account is told that account's index; it reads a skill whole when it
//! needs it, and never another account's.

use serde::{Deserialize, Serialize};

/// The record kind each skill is kept under, by its name.
pub const KIND: &str = "skill";
/// The record kind of the index of names and sentences, kept under `INDEX_KEY`.
pub const INDEX_KIND: &str = "skill-index";
pub const INDEX_KEY: &str = "index";

/// The most skills an account keeps: the index stays one record.
pub const MOST: usize = 200;
/// The most characters of the sentence on when a skill applies.
pub const DESCRIPTION_MOST: usize = 300;
/// The most bytes of a skill's instructions, within a record's limit.
pub const INSTRUCTIONS_MOST: usize = 60 * 1024;

/// One skill, as its record keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    /// When it applies, in a sentence: all a session is told of it until it
    /// reads it.
    pub description: String,
    /// What to do, in Markdown.
    pub instructions: String,
}

/// The names and sentences of an account's skills.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub skills: Vec<Listed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub description: String,
}

impl Skill {
    /// What is wrong with it, said as the interface says it.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_name(&self.name) {
            return Err(words::NAME.into());
        }
        let description = self.description.trim();
        if description.is_empty() || description.chars().count() > DESCRIPTION_MOST || description.contains('\n') {
            return Err(words::DESCRIPTION.into());
        }
        if self.instructions.trim().is_empty() || self.instructions.len() > INSTRUCTIONS_MOST {
            return Err(words::INSTRUCTIONS.into());
        }
        Ok(())
    }
}

/// A skill's name: lowercase letters, digits and hyphens, starting with a
/// letter, at most 64 characters, as hosts name their own.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// What a session is told of its account's skills: their names and when
/// each applies, and how to read one; nothing when there are none.
pub fn context(index: &Index) -> String {
    if index.skills.is_empty() {
        return String::new();
    }
    let mut text = String::from(words::INTRO);
    for skill in &index.skills {
        text.push_str(&format!("\n- {}: {}", skill.name, skill.description));
    }
    text
}

pub mod words {
    pub const INTRO: &str = "SKILLS\nInstructions the user keeps for kinds of work. Before work one of them applies to, read it with `fastctx skill NAME` and follow it.";
    pub const NAME: &str =
        "a skill's name is lowercase letters, digits and hyphens, starting with a letter, at most 64 characters";
    pub const DESCRIPTION: &str = "a skill needs a sentence on when it applies, on one line, at most 300 characters";
    pub const INSTRUCTIONS: &str = "a skill needs its instructions, at most 60 KiB";
    pub const TOO_MANY: &str = "an account keeps at most 200 skills";
    pub const ONE_ACCOUNT: &str = "only a conversation working for one account has skills";
    pub const USER_ONLY: &str = "skills are changed by the user, in the Fastctx app";
    pub const BUSY: &str = "the skills changed meanwhile; try again";
    pub fn missing(name: &str) -> String {
        format!("there is no skill {name}; the skills are listed in your context")
    }
}
