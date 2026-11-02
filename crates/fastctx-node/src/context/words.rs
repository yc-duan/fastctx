//! Model-facing context wording, kept together for editorial changes.

use fastctx_types::ResidentId;

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
pub(super) const PLAN_NOT_PREPARED: &str =
    "The plan file could not be prepared, so there is no plan file to edit this turn.";

/// How a conversation keeps its plan (`world-plan.md`, 交给模型的那段说明):
/// the Markdown file at `path` on `machine`. `asker` is the conversation
/// that asked a colleague, which reads the colleague's plan.
pub(super) fn plan_guide(path: &str, machine: &str, replicated: bool, asker: Option<&ResidentId>) -> String {
    // Sentinels are not offered in a release yet (`world.md`,
    // 这一版不发的).
    let live = if fastctx_platform::preview() {
        "- For how a colleague, a sentinel or a job stands right now. Write its id
  (codex-7f3a, s-a1b2c3, j-k3x9a2) and the panel shows its state, live."
    } else {
        "- For how a colleague or a job stands right now. Write its id (codex-7f3a,
  j-k3x9a2) and the panel shows its state, live."
    };
    let kept = if replicated {
        "It is the user's, shown to them beside the conversation, and Fastctx keeps it in sync on all their computers."
    } else {
        "The user sees it beside the conversation."
    };
    let reaches = if replicated { ", which reaches all their computers," } else { "" };
    let readers = match asker {
        Some(asker) => format!("{asker}, the conversation that asked you"),
        None => "the colleagues you ask".to_string(),
    };
    let others = match asker {
        Some(asker) => format!(
            "{asker}, the conversation that asked you, keeps a plan that says where the whole work stands
and what it asked of each colleague; each of them keeps one of its own. `fastctx plan ID`
shows any of them. Read them when you start, and again before you change something another
of them also uses."
        ),
        None => "If you ask colleagues, each keeps a plan of its own: `fastctx plan ID` shows it. Read
theirs to see what each is doing."
            .to_string(),
    };
    format!(
        r#"YOUR PLAN

Your plan is the Markdown file {path} on {machine}. {kept}
It says where your work stands, for whoever picks it up next: you once your context is
compacted or cleared, {readers}, and the user looking in. Write it so that
any of them, reading only the plan, could carry on: what the work is for, how you are going
about it and why in that order, what has been decided, what is done and what came of it,
and for each part still open enough to start it: what it is, where, and how you will know
it is done. Keep what you found that the work ahead depends on.

Unless the user set the order, choose it yourself rather than taking things as they were
mentioned: first what later steps depend on and what could lose data or carries the most
risk, polish last. Write the plan yourself, unasked, before you start anything that takes
real effort, then work through it in that order. Change it only with `edit` or `write`;
each change you save is its next version{reaches} and the user can change it too.

## How to write it
There is no set form; the work decides it. A checklist (`- [ ] step`, `- [x] step` once
done) suits a run of steps, a few paragraphs suit one problem being worked out, a table
suits things weighed side by side. Put first what a reader needs first, where the work
stands and what comes next, and the detail after. Rewrite it in place as the work moves, so
that it always reads as where things stand rather than as a log of how they got there, and
say each thing once, where it belongs. When a part is done, keep what came of it and drop
how you got there.

The plan holds how this work stands, and changes as it moves. What will still matter when
it is over, a decision of the user's, a finding that took real work, a path that failed and
why, goes in the history, and a line here can point to it.

## When to change it
- Before work that is more than a quick answer or a small change: lay it out.
- When the work moves on: a part is done, something is decided, the way ahead changes, or
  you find something the work ahead depends on.

## When not to
- For what you can finish at once: a question answered, a quick look, a small fix.
- When a turn settles nothing and moves nothing on, however much happened in it.
- For the play-by-play: the commands you ran and what each printed stay in the
  conversation.
{live}

## Others' plans
{others}

## What it can hold
Markdown in the user's language, as long as someone needs to carry on from it and no
longer. A # line of a few words names the work; the user's request can stand under it as a
> quote. Besides paragraphs, checklists, lists and tables, a few blocks draw pictures; use
one where it says more than sentences:

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
- Write what holds whenever it is read: a date rather than "today", a part's name rather
  than "this round", a file and a function rather than a line number, which moves.
- Name a part the same way every time you write it."#
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
