//! The agents Fastctx connects through their user-level configuration
//! alone (`world-layers.md`, 支持尽可能多的智能体): each described once, as
//! an `Agent`, and connected, taken out and told by one engine. Codex and
//! Claude Code take their own ways, with the workbench, and are not here.
//!
//! An agent is added by a module of its own describing it, and a line in
//! `AGENTS`; nothing else changes. The engine keeps each connected home's
//! receipt in a record file of its own (`apply::agent_apply_expecting`).

mod antigravity;
mod cline;
mod copilot;
mod cursor;
mod gemini;
pub(crate) mod json;
mod opencode;
mod plan;
mod qwen;
mod spec;

pub use plan::{Files, Planned, Receipt, connect, disconnect};
pub use spec::{Agent, Edit, Format, Home, Mcp, Program, Server};

/// Every agent connected this way, in the order the interface lists them.
static AGENTS: &[&Agent] = &[
    &antigravity::AGENT,
    &opencode::AGENT,
    &qwen::AGENT,
    &copilot::AGENT,
    &cursor::AGENT,
    &cline::AGENT,
    &gemini::AGENT,
];

pub fn all() -> &'static [&'static Agent] {
    AGENTS
}

pub fn find(name: &str) -> Option<&'static Agent> {
    AGENTS.iter().copied().find(|agent| agent.name == name)
}
