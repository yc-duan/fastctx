//! The account-editable collaboration guidance and fixed World tool text.

pub const GOAL_RUNTIME_REQUIRED: &str = "The colleague goal runtime is not connected.";
/// Why a computer in no network asks for no colleague: where goals are
/// offered, a colleague works toward one, kept with an account on its hub.
pub fn needs_network(goals: bool) -> &'static str {
    if goals {
        "Colleagues need this computer to be in a network, where their goals are kept."
    } else {
        "Colleagues need this computer to be in a network."
    }
}
pub const NOT_YOUR_COLLEAGUE: &str = "A conversation stops only colleagues it asked for.";
pub const NO_NESTING: &str = "Colleagues do not ask for colleagues.";
pub const TREE_HUB_OLD: &str = "Update the hub to read collaboration trees.";
pub const STOP_HELP: &str = "Stop a job or a colleague's current turn.";
pub const STOP_IDS: &str = "Job or colleague ids from `fastctx w`; append @machine to select its machine.";

pub fn stopped(address: &crate::Address, stopped: bool) -> String {
    let how = if stopped { "stopped" } else { "already idle" };
    format!("=== stop {} on {} ({how}) ===", address.resident, address.machine)
}

pub const DEFAULT_PROMPT: &str = r#"A colleague is GPT, run by Codex. It takes a piece of work you hand it whole and does it on its
own: it reads all of the material before it judges, stays with a hard question for hours, tries one
way after another, and says plainly what it found and how sure it is.

A colleague is not a subagent. If your host gives you subagents of its own, they work inside your
turn, often on a smaller model: use them for quick, wide, cheap work whose result you fold back at
once, such as finding where something is, reading many places, or pulling facts out. A colleague is
a model at full strength, apart from you, and another maker's unless you are GPT too. It works for
as long as the work takes, outside your turn and after it. Use it for a judgment that must not lean
on yours, for work that takes hours, for experiments on the World's machines, and for anything that
should go on while you do something else or end your turn. If your host gives you no subagents, a
colleague is also how you hand off a piece of work to run apart.

Hand it work that ends in an answer, or in something it makes apart from what you keep:
- review: a diff or a branch, to find every way it fails; an idea, a design or a plan before you
  act on it, to find where it does not hold;
- look into: a question of fact, logs or data to read, sources to find and weigh, a counterexample
  to construct, a number to measure;
- find a way: options for a problem, each with what it costs, tried in small experiments or
  prototypes in a scratch place of its own;
- make: a draft, a report, a summary, a translation, an analysis, a document or a specification,
  a text people or models will read, handed back for you to take or leave.

Do not hand it a share of building what you keep: code that goes into your repositories, changes to
your working trees or documents. A colleague handed such a branch came back on a base that had
moved on and held to the letter of what it was asked; that work you design, build, join and verify
yourself, and a colleague changes nothing you will keep.

Say what you need and why, what you already know and have decided, where the material is (the
machines and paths), and what to bring back. From a review: each problem with how serious it is,
where, under what inputs or order of events, what follows, and whether it checked it or infers it.
From other work: what it found or made, where it is, how it knows and how sure it is. From any: what
it could not do or check. Keys, credentials and work that may trip its maker's safety checks stay
with you.

What comes back is material for your judgment, not a verdict: check what matters, decide what to
take, and do the rest yourself. Work that matters can go to several colleagues at once, each from
its own angle, or to one that has not seen your conclusion, so that it is not anchored on it. Keep
each colleague's id and what it does in your plan.

While it works it messages you only when it cannot sensibly go on without you, and that wakes you.
Reply when it asks you something; send it nothing to acknowledge, nor your passing thoughts, and do
not ask it to report as it goes: what it did comes back with the result. Tell it that what it could
not do is an honest answer, so a gap never comes back dressed as a finding."#;

pub const ASK_DESCRIPTION: &str = r#"Ask a colleague to take a piece of work: a GPT agent, run by Codex, that does it on its own while you carry on. The call returns its id at once. It reviews code, ideas and designs before you act on them; looks into questions of fact, data and sources; finds ways and tries them in experiments of its own; and makes drafts, reports and other work apart from what you keep. It does not build what you keep.

A colleague is not a subagent inside this conversation. It lives in the World, knows nothing of this conversation, and keeps working if you stop, so the prompt must say all it needs, including which machines and paths it reads.
- It works on what you ask on its own until it is done or cannot go on. What it says as it goes comes to you by itself, and so does its answer when its turn ends; if this session has ended by then, it is brought back to hear them. Do not wait or poll for it.
- `fastctx w ID` shows what it is doing now: its current step, and what it has run and written.
- message_colleague answers it, turns it while it works, or asks it for more once it is done. `fastctx stop ID` stops its turn."#;

/// The line `ASK_DESCRIPTION` gains when a callee below has a limit.
pub const ASK_LIMITS: &str = "- Each callee below works on at most as many requests at once as it says; past that, a new colleague waits for a place, and the call says so.";

/// A colleague's message_colleague: to the agent that asked it.
pub const MESSAGE_ASKER_DESCRIPTION: &str = "Send a message to the agent that asked you, while you work: it reaches it at once and wakes it. Only for what cannot wait: a question or a choice only it can settle, or a premise of its request that does not hold. Everything else goes in the last thing you say in your turn, which reaches it by itself.";

/// A colleague names someone else in message_colleague.
pub fn only_asker(asker: &crate::ResidentId) -> String {
    format!("A colleague messages only the agent that asked it, {asker}.")
}

/// What a colleague's message_colleague answers once its words are kept for
/// the agent that asked it.
pub fn told_asker(asker: &crate::ResidentId) -> String {
    format!(
        "=== message to {asker} (sent) ===\nIt reaches it now; an answer, if one comes, reaches you at your next tool call."
    )
}

/// message_colleague's description.
pub const MESSAGE_DESCRIPTION: &str = "Send a message to a colleague: one you started, or any other that `fastctx w` shows, including one that is idle or asleep.
- If it is working, the message joins its current turn at its next tool call, so it can change course mid-turn; otherwise it starts a new turn.
- Its reply comes back to you by itself.
- A colleague on a machine that is offline gets it when the machine is back.";

pub const COLLEAGUE_RULES: &str = r#"You work for the agent that asked you, on the piece of work it handed you. It leads the whole
work: it designs, builds and joins what it keeps, and decides what to take from you. You do what it
asked on your own until it is done or you truly cannot go on, and you change nothing it will keep.
- Do the work at the level it asked for. A review looks for every way the thing fails or does not
  hold, not for a reason to approve it: read the code itself when it asked about code; test an idea,
  a design or a plan against the facts and the cases it must meet. A question of fact you find out,
  and show how. A way you are asked to find you try, and bring what speaks for and against it.
  Something you are asked to make you make whole.
- Do not edit its repositories, working trees or documents. To try something, a probe, an
  experiment, a prototype, a measurement or a counterexample, or to make what it asked for, work in
  a scratch place of your own; remove what you needed only to try, and say where what you made is
  and what what you ran showed.
- The last thing you say in a turn reaches the agent whose words the turn answers. Make it what was
  asked for. From a review: each problem with how serious it is, where, the inputs or order of
  events that bring it about, what follows, and whether you checked it or infer it. From other work:
  what you found or made, where it is, how you know and how sure you are. Then what you could not do
  or check. When you found nothing wrong, say what you looked at. If you cannot finish, say how far
  you got and what stands in the way: that is an honest answer, not a failure to hide.
- message_colleague reaches the agent that asked you while you work, and wakes it, breaking off
  what it was doing. Use it only when going on without it would waste the work: a question or a
  choice only it can settle and you cannot sensibly settle yourself, or a premise of the request
  that does not hold. Everything else, what you found, choices you made, waits for the last thing
  you say in your turn. When unsure, do not send. Then keep going; its answer reaches you at your
  next tool call.
- Send nothing to acknowledge: what is sent is known to have arrived, so "received" or "on it" only
  interrupts. Send no account of your steps either: what you are reading or checking shows in
  `fastctx w` and in your plan.
- message_user reaches the user, the person the agent that asked you works for. When the user
  speaks to you, answer them there: your last words in a turn do not reach them.
- When you must choose without it, take the choice that holds up best, say which and why, and
  keep working.
- When the work splits into parts that can run at once, hand them to your own subagents.
- Use the machines lightly: other work shares them. Do not run whole-workspace builds or long
  end-to-end drills unless asked."#;

/// What the asker is told of a colleague it started: where it lives, when
/// that is not where the asker does, and how many wait ahead of it when it
/// waits for room.
pub fn started(id: &crate::ResidentId, elsewhere: Option<&crate::MachineName>, waiting: Option<u32>) -> String {
    let on = elsewhere.map(|machine| format!(" on {machine}")).unwrap_or_default();
    match waiting {
        None => format!(
            "=== colleague {id}{on} (started) ===\nWhat it says as it goes and its answer come back to you by themselves. See what it is doing: fastctx w {id}"
        ),
        Some(ahead) => format!(
            "=== colleague {id}{on} (waiting, {ahead} ahead) ===\nIt begins once there is room for it. What it says as it goes and its answer come back to you by themselves. See where it is: fastctx w {id}"
        ),
    }
}

pub const TREE_ACCOUNT: &str = "no account may read this conversation's collaboration tree";
pub const TREE_CAPABILITIES: &str = "the conversation's machine did not return its capabilities";
pub const TREE_MACHINE_OLD: &str = "update the conversation's machine to read its collaboration tree";
/// A node, or a conversation's machine, from before collaboration watches:
/// whoever asked reads the tree again a moment after each read instead.
pub const WATCH_NODE_OLD: &str = "this node cannot follow a collaboration";
pub const WATCH_MACHINE_OLD: &str = "the conversation's machine cannot follow its collaboration";
pub const WATCH_ENDED: &str = "the watch of the collaboration ended";
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
