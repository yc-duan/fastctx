//! Model-facing context wording, kept together for editorial changes.

pub(super) fn changed(name: &str, by: &str) -> String {
    format!("{by} changed the {name} context; this replaces the previous version.")
}

pub(super) fn restored(name: &str) -> String {
    format!("The current {name} context replaces the version in your opening instructions.")
}

pub(super) fn removed(name: &str) -> String {
    format!("The {name} context is now empty; the earlier version no longer applies.")
}

pub(super) fn truncated(shown: usize, total: usize) -> String {
    format!("\n[Context truncated: the first {shown} of {total} UTF-8 bytes are shown.]")
}

pub(super) fn heading(name: &str, text: &str) -> String {
    format!("=== {name} ===\n{text}")
}

pub(super) const PLAN_UNAVAILABLE: &str = "The plan is unavailable.";

/// How a conversation keeps its plan (`world-plan.md`, 交给模型的那段说明).
pub(super) const PLAN_GUIDE: &str = r#"YOUR PLAN

Plan your own work here: what the work is, its steps in the order you will
take them, what has been decided, and where the work stands. Write it
yourself, unasked, before you start anything that takes real effort, then
work through it in that order, one step finished before the next is taken
up. The user sees your plan in a panel next to the conversation and can
change it; the colleagues you ask can read it. It is a canvas, not a form:
what you put on it, and how, is yours to choose. `fastctx plan` shows it;
write the whole new page to a draft file, then `fastctx plan --file <draft>`
puts it up as the next version.

## When to write it
- Before work that is more than a quick answer or a small change: lay out
  its steps in order.
- When the plan itself changes: a new decision, a step added, dropped or
  moved.
- When a step is done and the work stands at the next one.

## When not to
- For what you can finish at once: a question answered, a quick look, a
  small fix.
- Between steps. Change it only when the work has really moved, not after
  each command or for what you are about to try.
- For how a colleague, a sentinel or a job stands right now. Write its id
  (codex-7f3a, s-a1b2c3, j-k3x9a2, or a journal entry such as
  20261022T031205Z-desktop) and the page shows its state, live. A job going
  from 30% to 70%, a colleague starting or finishing a turn, a sentinel still
  watching: none of these is a reason to change the plan.
- To tick something off. Rewrite or delete what no longer holds.

## What it can hold
Markdown in the user's language, short enough to take in at a glance. A #
line of a few words names the work; the user's request can stand under it
as a > quote. Besides paragraphs, lists and tables, a few blocks draw
pictures; use one where it says more than sentences:

  ```flow       the steps, best as one chain on one line: A -> B -> C
                *B marks the step the work stands at; moving it is progress.
                A -> B: note   says something about the way into B.
                B: note        on a line of its own says something about B.
                A --> B        a way not yet sure.
                A -> B and A -> C make a fork: B and C may follow A.
  ```timeline   when things happen, one per line: 10-23 08:00 | what | detail
                *first marks the one that matters next.
  ```tree       a whole and its parts, by indentation: part | note
  ```ask        a decision you need from the user: the question on the first
                line, then "* option" for the one you suggest and "- option"
                for others. Their answer comes to you as their words; then
                write the decision in and take the question out.
  ```note warn  a risk or a blocker in a sentence or two (also note ok, note info)

## Tips
- Read the plan before you change it; the user may have changed it.
- Name a step the same way every time you write it.
- Keep it to what someone needs to see; the conversation holds the rest."#;

pub(super) const USER: &str = "The user";
pub(super) const UNKNOWN: &str = "unknown conversation";
pub(super) const UNAVAILABLE: &str = "the conversation is unavailable";
pub(super) const INVALID_NAME: &str = "invalid context block name";
pub(super) const TOO_MANY: &str = "a conversation can keep at most eight context blocks";
pub(crate) const REVISION_EXHAUSTED: &str = "context revision exhausted";
pub(super) const EPOCH_EXHAUSTED: &str = "context epoch exhausted";
pub(crate) const CODEX_PENDING: &str = "Codex has not made this thread's persisted context available";
pub(crate) fn codex_read(error: impl std::fmt::Display) -> String {
    format!("reading Codex compactions: {error}")
}
