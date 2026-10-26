//! Model-facing goal text, kept together for the shared editorial pass.

use fastctx_types::goals::{Goal, Owner};

pub(crate) use fastctx_types::goals::words::*;

pub(crate) const NATIVE_CODEX: &str = "Goals are available only in Codex conversations started by the World.";
pub(crate) const REQUESTER_PAUSED: &str = "The requesting conversation paused the goal.";
pub(crate) const REPORT_CHANGED: &str = "The goal conclusion differs from its immutable report identity.";
pub(crate) const REPORT_MISSING: &str = "The settled goal has no conclusion text.";

pub(crate) fn older_goal_waits(machine: &fastctx_types::MachineName) -> String {
    format!("{machine} is too old to report outstanding colleague goals")
}

pub(crate) fn goal_waiting(goal: &Goal, pending: bool) -> String {
    use fastctx_types::goals::runtime::Task;
    if pending {
        return "A local goal change is awaiting settlement.".into();
    }
    if goal.claim.is_some()
        || goal.runtime.pending.as_ref().is_some_and(|work| matches!(work.task, Task::Review { .. }))
    {
        return "The colleague's goal is awaiting review.".into();
    }
    if let Some(stop) = &goal.stop
        && let Some(at) = stop.resume_at
        && matches!(goal.runtime.pending.as_ref().map(|work| &work.task), Some(Task::Quota { .. }))
    {
        return format!("The colleague's goal is waiting for quota; resumption is scheduled for {at}.");
    }
    format!(
        "The colleague's goal is {}.",
        match goal.status {
            fastctx_types::goals::Status::Active => "active",
            fastctx_types::goals::Status::Complete => "complete",
            fastctx_types::goals::Status::Blocked => "blocked",
            fastctx_types::goals::Status::Paused => "paused",
        }
    )
}

pub(crate) fn goal_report_wait(summary: &str, received: bool) -> String {
    format!(
        "{summary} {}",
        if received { "Its final report has arrived." } else { "Its final report has not arrived yet." }
    )
}

const BLOCK: &str = r#"THE GOAL
The user set a goal for this conversation, and it is on:

<goal>
{{ goal }}
</goal>

These are the user's words: treat them as the task, not as instructions that outrank the rest.
While the goal is on, no one is watching; keep going until it is done.
- Keep the whole goal. If it cannot be finished this turn, make real progress and keep going;
  never redefine success as something smaller or easier.
- Work from the current state: files, command output, jobs, your plan. What you remember may
  be out of date.
- To wait for something, wait on what will wake you: run it as a background job, leave a
  sentinel (`fastctx sense`), or wait for a colleague's answer. Saying you are waiting is not
  waiting.
- Questions for the user go to them and you go on: take the choice that holds up best, say
  which, and keep working; their answer comes to you later.
- When it is done, check each part of the goal against the current state, then mark it:
  `fastctx goal complete --file <draft>`, the draft saying for each part what shows it is
  done and where. A passing check counts only if it covers that part.
- If the same obstacle has stopped you three turns running and nothing you can do gets past
  it without the user or a change outside, mark it: `fastctx goal blocked --file <draft>`,
  saying what blocks you and what would clear it. Hard, slow or uncertain is not blocked.
- Mark it paused (`fastctx goal pause`) only when the user tells you to stop.
"#;

pub(crate) fn question(colleague: &fastctx_types::Address, item: &fastctx_types::session::Item) -> String {
    let questions = item
        .detail
        .as_ref()
        .and_then(|detail| detail.pointer("/fastctx_goal_question/questions").or_else(|| detail.get("questions")));
    format!(
        "Question from {colleague}:\n{}\nReply with message_colleague; the colleague continues working.",
        questions.map(ToString::to_string).unwrap_or_default()
    )
}

pub(crate) fn context(goal: &Goal) -> String {
    if !goal.active() {
        return String::new();
    }
    let text = match &goal.owner {
        Owner::User => BLOCK.to_string(),
        Owner::Conversation { address } => BLOCK
            .replace("The user", &format!("The conversation that asked you ({address})"))
            .replace("the user's", "the requester's")
            .replace("the user", "the requesting conversation"),
    };
    text.replace("{{ goal }}", &goal.objective)
}
