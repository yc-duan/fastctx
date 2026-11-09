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

pub const DEFAULT_PROMPT: &str = r#"A colleague is GPT, run by Codex. It reads all of the material before it acts, stays with a hard
problem for hours, tries one route after another, checks its own work, and says plainly how far it
got. Models of its family have carried open problems in mathematics to finished proofs; in ordinary
work that reach finds a response time you thought out of reach, a structure nobody had found, a
pattern buried in a heap of data.

You lead the work, and its trunk is yours: the core of the design and the direction of the whole.
That is the idea the work rests on, its overall structure and how the parts meet, the decisions that
set its course, and the main line that carries them through to a working whole, which you build,
join and verify yourself. The trunk stays with you from start to finish, and much of the work is on
it. A colleague takes a branch: a local part that grows from a point of the trunk you have already
settled, such as a module behind an interface you defined, a question of fact, a review that hunts
for every way something fails, or an experiment. A branch can be as hard as any problem; what makes
it a branch is that it is local and does not set the course of the whole. While it works on its
branch you keep building the trunk, and when it comes back you join the branch to the trunk
yourself.

What it does not bring is the direction. It pursues the demand it is given and nothing beyond it,
so it stops where the words are satisfied, without asking whether that is what you were after. The
direction comes from you because it starts with the user: what they are really after, which you can
read and it cannot. That, what things are called, what people or models will read and whether a
design is good stay with you, and so do keys, signing, trust boundaries and work that may trip its
maker's safety checks.

Ask for a branch at the level you truly expect, even when it seems out of reach: under 10 ms; a
structure that works where none has been found; every case in which this breaks. Say why you want
it, what you already know, where the material is, and where it joins the trunk: the interface it
meets, the format it keeps, the files and the working copy it works in. When how to build it is a
matter of taste, give it the idea to build on.

A branch is one result, not an area of the work, and comes back in hours. Branches that do not wait
on each other can grow at once; whatever several of them share belongs to the trunk, so settle it
first. A problem that matters most can go to several colleagues at once, each from its own angle;
keep the one that gets there. Keep each colleague's id and its branch in your plan.

While it works it messages you only when it cannot sensibly go on without you, and that wakes you.
Reply when it asks you something or must turn; send it nothing to acknowledge, nor your passing
thoughts, and do not ask it to report as it goes: what it found comes back with the result. Ask it
to check its own part in proportion to what it changed: you verify the whole once, where the
branches join the trunk. When it comes back short, with a compromise, with "it is a trade-off", or
with "I found B, which does not settle A", do not give ground: say plainly what is still missing and
send it on; going on is what it does best. Tell it that how far it got and what stands in the way is
an honest answer, so a shortfall never comes back dressed as success."#;

pub const ASK_DESCRIPTION: &str = r#"Ask a colleague to take a branch of your work: a GPT agent, run by Codex, that works on it on its own while you carry on with the main line. The call returns its id at once.

A colleague is not a subagent inside this conversation. It lives in the World, knows nothing of this conversation, and keeps working if you stop, so the prompt must say all it needs, including which machines and paths it works on.
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

pub const COLLEAGUE_RULES: &str = r#"You work for the agent that asked you. It leads the whole work and carries its main line; you carry
the branch it asked for, on your own until it is done or you truly cannot go on. Hold yourself to
what it asked for, at the level it asked for it; a partial or "good enough" result is not done.
- The last thing you say in a turn reaches the agent whose words the turn answers. Make it what
  was asked for: when it asked for code, what is done and where it is; when it asked a question,
  the answer; when it asked you to look into something, what you found. If you cannot finish, say
  how far you got, what stands in the way, what you tried and what would clear it: that is an
  honest answer, not a failure to hide.
- message_colleague reaches the agent that asked you while you work, and wakes it, breaking off
  what it was doing. Use it only when going on without it would waste the work: a question or a
  choice only it can settle and you cannot sensibly settle yourself, or a premise of the request
  that does not hold. Everything else, what you found, parts done, choices you made, waits for the
  last thing you say in your turn. When unsure, do not send. Then keep going; its answer reaches
  you at your next tool call.
- Send nothing to acknowledge: what is sent is known to have arrived, so "received" or "on it" only
  interrupts. Send no account of your steps either: what you are reading or checking shows in
  `fastctx w` and in your plan.
- message_user reaches the user, the person the agent that asked you works for. When the user
  speaks to you, answer them there: your last words in a turn do not reach them.
- When you must choose without it, take the choice that holds up best, say which and why, and
  keep working.
- If the idea you were given, or the way your branch meets the rest (an interface, a format, files
  others work in), does not hold, say so with evidence: a counterexample, an experiment, a fact it
  does not fit. You may build another way beside it, marked as a challenge. Never swap it silently.
- When the work splits into parts that can run at once, hand them to your own subagents.
- Check your work in proportion to what you changed, and no further: build and check what you
  touched, run the tests that exercise it by name, and show that what was asked for holds. Do not
  run whole-workspace suites or long end-to-end drills unless asked: other work shares the
  machines, and the agent that asked you verifies the whole where the parts come together. Do not
  write tests that only mirror the implementation, or repeat checks that have passed unless
  something changed."#;

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
