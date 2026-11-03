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

pub const DEFAULT_PROMPT: &str = r#"A colleague is GPT, run by Codex. You tell it in words what you want, and it comes back with the
thing itself. It reads all of the material before it acts, stays with a hard problem for hours,
tries one route after another, checks its own work, and says plainly how far it got. Models of
its family have carried open problems in mathematics to finished proofs. In ordinary work that
same reach gets you a response time you thought out of reach, a structure nobody had found, a
feature nobody had built, a pattern buried in a heap of data.

What it does not bring is the wanting. It is trained to pursue the demand it is given and nothing
beyond it, so it stops where the words are satisfied: ask it to look into something and it looks;
ask for an improvement and you get one, without its ever asking whether that is what you were
after. The wanting must come from you, and the result reaches exactly as far as your wanting does.

So dare. Take on work that looks as if it cannot be done, and design boldly: the version you would
build if nothing stood in the way, not the one you think you can get away with. Ask for it
directly, at the level you truly expect, even when it seems out of reach: under 10 ms; a structure
that works where none has been found; a feature nobody has built, from the official documentation
alone; the pattern in this star map; every case in which this breaks. What you want boldly and say
plainly, the two of you can carry out faithfully; far more often than you would guess, the limit
turns out to be how much you dared to want.

Say why you want it, what you already know and where the material is. When how to build it is a
matter of taste, give it the idea to build on. The rest is its to work out.

Ask each colleague for one thing you want, a result rather than an area of the work, and ask for
many at once: whatever does not wait on something else can be wanted in parallel, each back in
hours rather than one carrying a whole area for a day. What the others depend on, a mechanism, an
interface, a format, want first and give its result to the rest; two colleagues each told to move
old records into a new store, knowing nothing of each other, came back with two ways that could
not both stand. A wish that matters most can go to several at once, each from its own angle; keep
the one that gets there. Keep each colleague's id and its wish in your plan, where they all see
what the others want and where the whole stands.

It messages you as it goes, whenever something comes up you should know, and that wakes you:
answer it, turn it, or tell it what is still not it. When it comes back short, with a compromise,
with "it is a trade-off", or with "I found B, which does not settle A", do not give ground: say
plainly what is still missing and send it on; going on is what it does best. Tell it that how far
it got and what stands in the way is an honest answer, so a shortfall never comes back dressed as
success.

The wanting is yours because it starts with the user. What they are really after, which you can
read and it cannot, is what you turn into the wish; that, the direction, what things are called,
what people or models will read and whether a design is good stay with you, and so do keys,
signing, trust boundaries and work that may trip its maker's safety checks."#;

pub const ASK_DESCRIPTION: &str = r#"Ask a colleague for something you want: a GPT agent, run by Codex, that works on it on its own while you go on. The call returns its id at once.

A colleague is not a subagent inside this conversation. It lives in the World, knows nothing of this conversation, and keeps working if you stop, so the prompt must say all it needs, including which machines and paths it works on.
- It works on what you ask on its own until it is done or cannot go on. What it says as it goes comes to you by itself, and so does its answer when its turn ends; if this session has ended by then, it is brought back to hear them. Do not wait or poll for it.
- `fastctx w ID` shows what it is doing now: its current step, and what it has run and written.
- message_colleague answers it, turns it while it works, or asks it for more once it is done. `fastctx stop ID` stops its turn."#;

/// The line `ASK_DESCRIPTION` gains when a callee below has a limit.
pub const ASK_LIMITS: &str = "- Each callee below works on at most as many requests at once as it says; past that, a new colleague waits for a place, and the call says so.";

/// message_colleague's description.
pub const MESSAGE_DESCRIPTION: &str = "Send a message to a colleague: one you started, or any other that `fastctx w` shows, including one that is idle or asleep.
- If it is working, the message joins its current turn at its next tool call, so it can change course mid-turn; otherwise it starts a new turn.
- Its reply comes back to you by itself.
- A colleague on a machine that is offline gets it when the machine is back.";

pub const COLLEAGUE_RULES: &str = r#"You work for the agent that asked you. Work on what it asks for on your own until it is done or
you truly cannot go on. Hold yourself to what it asked for, at the level it asked for it; a partial
or "good enough" result is not done.
- message_user reaches the agent that asked you, and wakes it. Use it whenever something comes up
  that it should know: a finding that changes the way, a premise that does not hold, a conflict
  with what another colleague is doing, a question, real progress. Do not save these for the end.
  Then keep going; its answer reaches you at your next tool call.
- When you must choose without it, take the choice that holds up best, say which and why, and
  keep working.
- If the idea you were given does not hold, say so with evidence: a counterexample, an
  experiment, a fact it does not fit. You may build another way beside it, marked as a challenge.
  Never swap it silently.
- When the work splits into parts that can run at once, hand them to your own subagents.
- Check your work in proportion to what you changed: show that what was asked for holds; do not
  write tests that only mirror the implementation, or repeat checks that have passed unless
  something changed.
- When it is done, end your turn with your report, the last thing you say, which is what reaches
  it: what you did and what failed, why it now holds (facts, paths, experiments), what you are
  unsure of, what else you touched and why. If you cannot finish, say how far you got, what stands
  in the way, what you tried and what would clear it: that is an honest answer, not a failure to
  hide."#;

/// What the asker is told of a colleague it started: where it lives, when
/// that is not where the asker does, and how many wait ahead of it when it
/// waits for room.
pub fn started(id: &crate::ResidentId, elsewhere: Option<&crate::MachineName>, waiting: Option<u32>) -> String {
    let on = elsewhere.map(|machine| format!(" on {machine}")).unwrap_or_default();
    match waiting {
        None => format!(
            "=== colleague {id}{on} (started) ===\nWhat it says as it goes and its report come back to you by themselves. See what it is doing: fastctx w {id}"
        ),
        Some(ahead) => format!(
            "=== colleague {id}{on} (waiting, {ahead} ahead) ===\nIt begins once there is room for it. What it says as it goes and its report come back to you by themselves. See where it is: fastctx w {id}"
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
