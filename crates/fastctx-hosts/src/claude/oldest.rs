//! The oldest Claude Code the World works with. Mods, which the World's
//! plugin is, came in 2.1.287; the plugin also relies on what 2.1.300
//! changed: `turn.start` for every subagent run, with its `agentId`, and a
//! plugin's own `prompt.submit` not reaching its own hooks. Nothing older is
//! connected, and no session is started in it.

/// The oldest Claude Code version the World's plugin runs in.
pub const OLDEST: &str = "2.1.300";

/// Whether the Claude Code that said `version` of itself, as `claude
/// --version` prints it (`2.1.303 (Claude Code)`) or as a bare number, is
/// one the World works with. A pre-release counts as its release. What says
/// no version the World can read is refused, with what it said.
pub fn supported(version: &str) -> Result<(), String> {
    let number = version.split_whitespace().next().unwrap_or_default();
    let oldest = fastctx_types::update::release(OLDEST).expect("a release version");
    match fastctx_types::update::release(number) {
        Some(found) if (found.major, found.minor, found.patch) >= (oldest.major, oldest.minor, oldest.patch) => Ok(()),
        Some(_) => Err(format!(
            "Claude Code {number} is older than {OLDEST}, the oldest the World works with; update Claude Code (claude update) and try again"
        )),
        None => Err(format!(
            "Claude Code did not say a version the World can read ({version:?}); the World works with {OLDEST} or later"
        )),
    }
}
