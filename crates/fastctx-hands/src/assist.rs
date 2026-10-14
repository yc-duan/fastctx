//! What an agent assisting another account is told (`world-run.md`, 这里):
//! it may not work on the computer it runs on, which goes unnamed, since a
//! model tries every machine it sees named; and so every command it runs
//! names its machine with `on`. Its host still tells it that computer's working
//! directory and platform, so these words overrule them plainly. They lead
//! both the `run` tool's description and the session's first instructions,
//! on purpose twice: it is the opposite of what a model takes for granted.

/// The opening for an assisting agent, with the machines it may work on.
pub fn opening(machines: &str) -> String {
    let reach = if machines.is_empty() {
        "You have access to no machine yet: the user has none you may work on, so no command of yours can run anywhere; tell the user.".to_string()
    } else {
        format!(
            "You have access to these machines: {machines}. Every command must name its machine with `on`: each script you run is a single `on MACHINE ...`, such as `on laptop cargo test`, or `on laptop <<'EOF'`, then the script, then `EOF`. To work on several machines in one script, put the script inside one `on` and use `on` again inside it."
        )
    };
    format!(
        "You have no access to the computer this program runs on: the user may use this agent, not that computer. The working directory, platform and paths your host reports are that computer's, and your shell never runs there. Guidance that calls that computer one of your machines, or says `run` starts on it, was written for its owner's own sessions and does not apply to you.\n{reach}"
    )
}

/// The answer to a script of an assisting agent that is not a single `on`.
pub const SCRIPT_REFUSED: &str = "nothing ran: you have no access to the computer this program runs on, so every command must name its machine with `on`, and this script is not a single `on MACHINE ...`. Write `on MACHINE COMMAND`, or `on MACHINE <<'EOF'`, then the script, then `EOF`; to work on several machines, put the script inside one `on` and use `on` again inside it.";

/// The answer to an `on` of an assisting agent that names the computer it
/// runs on.
pub fn here_refused(machine: &str) -> String {
    format!(
        "nothing ran: {machine} is the computer this program runs on, and you have no access to it; name one of the machines you have access to."
    )
}
