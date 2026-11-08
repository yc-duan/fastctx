//! What those who hold a machine changed in the models the World offers in
//! one of its Claude Code logins (`world-gui.md`, 对话): the World's own
//! table less the rows they took out, then the model ids they added. The
//! change is kept against the table, so a row a later release adds is
//! offered without anyone adding it, and a row taken out stays out. An id
//! added stays offered when a later table holds it, or holds it under a
//! family the login is not seen to offer.

use serde::{Deserialize, Serialize};

/// The most model ids one login takes beyond the World's table.
pub const MOST_ADDED: usize = 64;
/// The longest model id taken, in bytes.
pub const MOST_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEdits {
    /// Rows of the World's table taken out, by full model id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Model ids added, in the order they were added; offered after the
    /// table's rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
}

/// One change of a login's models.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum ModelChange {
    /// Offers `model`: a row taken out is offered again, another id is added.
    Add { model: String },
    /// No longer offers `model`.
    Remove { model: String },
    /// Offers the World's own table again, as it comes.
    Reset,
}

impl ModelEdits {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }

    /// Applies `change`. `in_table` says whether a model id is a row of the
    /// World's table, `by_family` whether the table offers it to this login
    /// by itself, its family being one the login offers.
    pub fn apply(
        &mut self,
        change: &ModelChange,
        in_table: impl Fn(&str) -> bool,
        by_family: impl Fn(&str) -> bool,
    ) -> Result<(), String> {
        match change {
            ModelChange::Add { model } => {
                check(model)?;
                self.removed.retain(|id| id != model);
                if !by_family(model) && !self.added.contains(model) {
                    if self.added.len() >= MOST_ADDED {
                        return Err(format!("a login takes at most {MOST_ADDED} custom models"));
                    }
                    self.added.push(model.clone());
                }
            }
            ModelChange::Remove { model } => {
                let added = self.added.contains(model);
                self.added.retain(|id| id != model);
                if in_table(model) {
                    if !self.removed.contains(model) {
                        self.removed.push(model.clone());
                    }
                } else if !added {
                    return Err(format!("{model} is not among this login's models"));
                }
            }
            ModelChange::Reset => *self = Self::default(),
        }
        Ok(())
    }
}

/// A model id as the World passes it to Claude Code's `--model`: printable
/// ASCII without spaces, not read as an option.
pub fn check(model: &str) -> Result<(), String> {
    if model.is_empty() {
        return Err("a model id is not empty".into());
    }
    if model.len() > MOST_ID_BYTES {
        return Err(format!("a model id is at most {MOST_ID_BYTES} bytes"));
    }
    if !model.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(format!("{model:?} is not a model id: letters, digits and punctuation, without spaces"));
    }
    if model.starts_with('-') {
        return Err(format!("{model} is not a model id: it would be read as an option"));
    }
    Ok(())
}
