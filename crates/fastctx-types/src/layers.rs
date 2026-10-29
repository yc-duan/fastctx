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

/// The layer an account chose; none until it chooses, which the first
/// interface opened asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::tolerant::read_option")]
    pub layer: Option<Layer>,
}

impl Chosen {
    /// What the World works by: an account that has not chosen keeps the
    /// workbench it had before there were layers.
    pub fn effective(&self) -> Layer {
        self.layer.unwrap_or(Layer::Workbench)
    }
}

/// Said when the World is asked to start or drive a conversation for an
/// account in Connect.
pub const CONNECT_ONLY: &str = "this account uses Fastctx to connect its own agents only; conversations in Fastctx need the workbench, chosen in Settings";

/// Said when an account's hub is too old to keep a layer.
pub const HUB_TOO_OLD: &str = "the hub is too old to keep the account's layer; update the hub's machine";
