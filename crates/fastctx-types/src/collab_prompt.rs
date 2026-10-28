//! The account-editable collaboration guidance and fixed World tool text.

pub const GOAL_RUNTIME_REQUIRED: &str = "The colleague goal runtime is not connected.";
/// A colleague works toward a goal, and goals are kept with an account on its hub.
pub const GOAL_NEEDS_NETWORK: &str = "Colleagues need this computer to be in a network, where their goals are kept.";
pub const NOT_YOUR_COLLEAGUE: &str = "A conversation stops only colleagues it asked for.";
pub const NO_NESTING: &str = "Colleagues do not ask for colleagues.";
pub const TREE_HUB_OLD: &str = "Update the hub to read collaboration trees.";
pub const STOP_HELP: &str = "Stop a job or a colleague's current turn.";
pub const STOP_IDS: &str = "Job or colleague ids from `fastctx w`; append @machine to select its machine.";

pub fn stopped(address: &crate::Address, stopped: bool) -> String {
    let how = if stopped { "stopped" } else { "already idle" };
    format!("=== stop {} on {} ({how}) ===", address.resident, address.machine)
}

pub const DEFAULT_PROMPT: &str = r#"When to ask one:
- GPT is strong on a part and weak on the whole. Within one part it reads all the material before
  acting, works a hard point for hours, changes approach when one fails, and says plainly when it
  cannot do something. Asked to judge a whole that has not taken shape, or to build a whole thing
  from nothing, it returns pieces that are each careful and have no idea holding them together.
- So keep the whole yourself: what the user means, the structure, what things are called, where it
  goes next. Hand out parts as they come up: code whose need you can state and whose result facts
  can judge; something that must be airtight; a behaviour nobody understands yet; a detail that
  must be exactly right; an exhaustive check of an outside fact; a counterexample to construct; an
  experiment or measurement that answers a definite question.
- Ask when you are about to read through a pile of material, try a set of cases, or make something
  airtight. Being able to do it in a few minutes is no reason to keep it.
- Keep for yourself: judging whether a design or a result is good, which it does by matching words;
  taste, such as text a model will read or an interface; keys, signing, encryption and trust
  boundaries; work that may trip its maker's safety checks.

Writing the prompt:
- Say what this part needs, why, and where things stand, with the facts you have checked. Point to
  the material rather than summarizing it.
- For a part that builds something, give its shape: the one idea it rests on and what must hold, in
  a sentence or two. Without a shape it invents one, usually a worse one.
- Say what must be true when it is done, as facts that can be checked.
- Say where it works: which machine and paths, and a copy of its own, such as a branch or a
  worktree, when it writes code; and the limits of anything shared, such as a machine's memory or
  build jobs.
- Do not limit which files it may touch or which checks it must pass, and do not tell it how to
  work: that is where it is strongest.
- To learn a fact, ask a question facts can answer, not for its opinion of a plan.

While it works:
- Its answer comes back to you by itself; do not wait or watch for it. Write its id in your plan and
  the plan shows how it is doing.
- Leave it alone unless something it depends on changed. Answer its step-level questions yourself,
  or tell it to take its own suggestion.

When it reports:
- Check the claims you will build on. If it answered a different question, ask again: does it hold,
  yes or no, and what is the evidence?
- Look for two things: whether it went around the shape you gave, and whether it piled up special
  cases instead of one idea.
- One part, one colleague; several can work at once, each in its own copy, and you merge."#;

pub const ASK_DESCRIPTION: &str = r#"Start a colleague on a part of the work: a GPT agent, run by Codex, that works on its own while you go on. The call returns its id at once.

A colleague is not a subagent inside this conversation. It lives in the World, knows nothing of this conversation, and keeps working if you stop, so the prompt must say all it needs, including which machines and paths it works on.
- What you ask is its goal: it keeps going on its own until it is done or cannot go on. Then its report comes back to you by itself, and so does its reply each time you message it. If this session has ended by then, it is brought back to hear it. Do not wait or poll for it.
- Its questions come to you while it goes on with the choice that holds up best; answer with message_colleague.
- `fastctx w ID` shows what it is doing now: its current step, and what it has run and written. With its id in your plan, the plan shows it too.
- message_colleague tells it more while it works, or gives it the next part when it is done. `fastctx stop ID` stops its turn."#;

pub const MESSAGE_DESCRIPTION: &str = r#"Send a message to a colleague: one you started, or any other that `fastctx w` shows, including one that is idle or asleep.
- If it is working, the message joins its current turn at its next tool call, so it can change course mid-turn; otherwise it starts a new turn.
- Its reply comes back to you by itself. If it marked its goal blocked, your message starts it again.
- A colleague on a machine that is offline gets it when the machine is back."#;

pub const COLLEAGUE_RULES: &str = r#"You work for the agent that asked you. What it asks is your goal, and the goal is on: keep going
on your own until it is done or you truly cannot go on. `fastctx plan` shows its plan.
- Questions for it go to it while you go on: take the choice that holds up best, say which and
  why, and keep working; its answer comes to you later.
- If the shape you were given does not hold, say so with evidence: a counterexample, an experiment,
  a fact it does not fit. You may build another way beside it, marked as a challenge. Never swap it
  silently.
- When it is done, mark it: `fastctx goal complete --file <draft>`. The draft is your report: what
  you did and what failed, why the part now holds (facts, paths, experiments), what you are unsure
  of, what else you touched and why, and what the whole should know.
- If you cannot go on, mark it: `fastctx goal blocked --file <draft>`, saying what blocks you, what
  you tried and what would clear it.
- The project's journals and specs are for you to read, not to write."#;

pub fn started(id: &crate::ResidentId, machine: &crate::MachineName) -> String {
    format!(
        "=== colleague {id} on {machine} (started) ===\nIts report comes back to you when it is done or cannot go on. See what it is doing: fastctx w {id}"
    )
}

pub const TREE_ACCOUNT: &str = "no account may read this conversation's collaboration tree";
pub const TREE_CAPABILITIES: &str = "the conversation's machine did not return its capabilities";
pub const TREE_MACHINE_OLD: &str = "update the conversation's machine to read its collaboration tree";
pub const TREE_UNANSWERED: &str = "the conversation's machine did not return its collaboration tree";
pub const TREE_INVISIBLE: &str = "the conversation is not visible to this account";
pub const TREE_DEADLINE: &str = "the collaboration query deadline was reached";
pub const GOAL_DEADLINE: &str = "the goal state was not read before the query deadline";
pub const NATIVE_CYCLE: &str = "the host reported a cycle in agent parents";
pub const NATIVE_DEADLINE: &str = "reading native agents exceeded the query deadline";
pub const NATIVE_HISTORY_DEADLINE: &str = "its history was not read before the query deadline";
pub const STOP_IDENTIFY: &str = "the colleague's machine did not identify it";
pub const STOP_UNANSWERED: &str = "the colleague's machine did not answer the stop";

pub fn recorded_colleague(error: impl std::fmt::Display) -> String {
    format!("invalid recorded colleague: {error}")
}

/// A machine of an earlier release would start a colleague without a goal.
pub fn colleague_host_old(machine: impl std::fmt::Display) -> String {
    format!("update {machine} to ask a colleague there")
}

pub fn goal_failed(id: &crate::ResidentId, failure: impl std::fmt::Display) -> String {
    format!("colleague {id} was created, but its goal could not be opened: {failure}")
}

pub fn first_request_failed(id: &crate::ResidentId, error: impl std::fmt::Display) -> String {
    format!("colleague {id} exists, but keeping its first request failed: {error}")
}
