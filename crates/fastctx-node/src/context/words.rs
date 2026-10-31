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

/// Why this turn has no plan file, when its working computer or the hub
/// cannot keep one, or it could not be prepared.
pub(super) const PLAN_OLD_COMPUTER: &str = "The computer this conversation works from runs an older Fastctx that cannot keep the plan file, so there is no plan file to edit until it is updated.";
pub(super) const PLAN_OLD_HUB: &str =
    "The account's hub runs an older Fastctx, so there is no plan file to edit until it is updated.";
pub(super) const PLAN_NOT_PREPARED: &str = "The plan file could not be prepared, so there is no plan file to edit this turn.";

/// How a conversation keeps its plan (`world-plan.md`, 交给模型的那段说明).
/// How to keep a plan, for a conversation that keeps its own: the plan is
/// the Markdown file at `path` on `machine` (`plan_file`).
pub(super) fn plan_guide(path: &str, machine: &str) -> String {
    // Sentinels and journals are not offered in a release yet (`world.md`,
    // 这一版不发的).
    let live = if fastctx_platform::preview() {
        "- For how a colleague, a sentinel or a job stands right now. Write its id
  (codex-7f3a, s-a1b2c3, j-k3x9a2, or a journal entry such as
  20261022T031205Z-desktop) and the panel shows its state, live."
    } else {
        "- For how a colleague or a job stands right now. Write its id (codex-7f3a,
  j-k3x9a2) and the panel shows its state, live."
    };
    format!(
        r#"YOUR PLAN

Your plan is the Markdown file {path} on {machine}. Plan your own work in it:
what the work is, its steps in the order you will take them, what has been
decided. Unless the user set the order, choose it yourself rather than
taking things as they were mentioned: first what later steps depend on and
what could lose data or carries the most risk, polish last. Write it
yourself, unasked, before you start anything that takes real effort, then
work through it in that order. Change it as you change
any file, with `edit`, `write` or anything else, on {machine}; each change
you save is its next version, which the user sees in a panel beside the
conversation and can change too. The colleagues you ask can read it.

## Progress
Progress is a checklist: `- [ ] step` for each step, `- [x] step` once it is
done. Tick a step when it is done; add, reword or drop steps as the work
changes. Nothing else marks where the work stands.

## When to change it
- Before work that is more than a quick answer or a small change: lay out
  its steps.
- When a step is done, or the plan itself changes: a decision, a step
  added, dropped or moved.

## When not to
- For what you can finish at once: a question answered, a quick look, a
  small fix.
{live}

## What it can hold
Markdown in the user's language, short enough to take in at a glance. A #
line of a few words names the work; the user's request can stand under it
as a > quote. Besides paragraphs, checklists, lists and tables, a few blocks
draw pictures; use one where it says more than sentences:

  ```flow       how the steps connect, best as one chain: A -> B -> C
                A -> B: note   says something about the way into B.
                A --> B        a way not yet sure.
                A -> B and A -> C make a fork: B and C may follow A.
  ```timeline   when things happen, one per line: 10-23 08:00 | what | detail
  ```tree       a whole and its parts, by indentation: part | note
  ```ask        a decision you need from the user: the question on the first
                line, then "* option" for the one you suggest and "- option"
                for others. Their answer comes to you as their words; then
                write the decision in and take the question out.
  ```note warn  a risk or a blocker in a sentence or two (also note ok, note info)

## Tips
- Read the file before you change it; the user may have changed it.
- Name a step the same way every time you write it."#
    )
}

pub(super) const USER: &str = "The user";
pub(super) const UNKNOWN: &str = "unknown conversation";
pub(super) const UNAVAILABLE: &str = "the conversation is unavailable";
pub(super) const INVALID_NAME: &str = "invalid context block name";
pub(super) const TOO_MANY: &str = "a conversation can keep at most twelve context blocks";
pub(crate) const REVISION_EXHAUSTED: &str = "context revision exhausted";
pub(super) const EPOCH_EXHAUSTED: &str = "context epoch exhausted";
pub(crate) const CODEX_PENDING: &str = "Codex has not made this thread's persisted context available";
pub(crate) fn codex_read(error: impl std::fmt::Display) -> String {
    format!("reading Codex compactions: {error}")
}
