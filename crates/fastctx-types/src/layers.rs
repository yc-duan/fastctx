//! The two layers of Fastctx (`world-layers.md`): Connect gives the user's
//! own agents `run` and the activity of their machines; Workbench is Connect
//! with the World's conversations, whose agents are its kernels. An account
//! keeps its layer on its hub; a computer in no network keeps its own.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// The user works in their own Claude Code and Codex; the World answers
    /// their calls and starts nothing.
    Connect,
    /// The World's conversations as well.
    Workbench,
}

impl crate::tolerant::Tolerant for Layer {
    const ALL: &'static [Self] = &[Self::Connect, Self::Workbench];
    const UNKNOWN: Self = Self::Connect;
}

/// The layer an account chose; none until it chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::tolerant::read_option")]
    pub layer: Option<Layer>,
}

impl Chosen {
    /// What the World works by: the layer chosen. An account that never
    /// chose is in Connect, unless `held` says Fastctx holds conversations
    /// of its own for it from before there were layers: those keep the
    /// workbench they had.
    pub fn effective(&self, held: bool) -> Layer {
        self.layer.unwrap_or(if held { Layer::Workbench } else { Layer::Connect })
    }
}

/// Said when an account's hub is too old to keep a layer.
pub const HUB_TOO_OLD: &str = "the hub is too old to keep the account's layer; update the hub's machine";

/// Whether the workbench's places are shown, from what each computer that
/// could have it open in a login told (`world-layers.md`): shown while one
/// has it open, put away once every one told it has not; none while one has
/// not told, as an earlier release or one not heard yet, when the account's
/// layer decides.
pub fn shown(told: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut closed = true;
    for one in told {
        match one {
            Some(true) => return Some(true),
            Some(false) => {}
            None => closed = false,
        }
    }
    closed.then_some(false)
}
