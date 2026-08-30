//! Recognition of the host's `apply_patch` editing channel being run as a shell program.

const CHANNEL_NAME: &str = "apply_patch";
const MISUSE_NOTE: &str = "apply_patch is not a shell program; use the host's editing channel";

/// Returns the guidance note for a failed command that reads as an attempt to run `apply_patch`.
///
/// The host resolves `apply_patch` on its own shell channel before any real bash sees it, so the
/// word only ever reaches a shell when it is routed through a tool the host does not inspect —
/// and bash then reports it as a missing command. The note stays tentative and never blocks the
/// command, because a user may legitimately own an executable by that name: it appears only after
/// the command has actually failed, so a working script is never interrupted or second-guessed.
pub(crate) fn misuse_note(
    command: &str,
    exit_code: i32,
    timeout_ms: Option<u64>,
) -> Option<String> {
    let failed_on_its_own = timeout_ms.is_none() && exit_code != 0;
    (failed_on_its_own && invokes_channel(command)).then(|| MISUSE_NOTE.to_string())
}

/// Reports whether any command position in the line starts the bare `apply_patch` word.
///
/// Splitting on shell separators keeps `echo apply_patch` and `/usr/bin/apply_patch` out: the
/// first is not in command position, the second names a real executable the user does own.
fn invokes_channel(command: &str) -> bool {
    command.contains(CHANNEL_NAME)
        && command
            .split([';', '&', '|', '\n', '(', '{'])
            .any(|segment| leading_word(segment) == CHANNEL_NAME)
}

/// Extracts the first word of a segment, stopping at whitespace or a redirection operator.
fn leading_word(segment: &str) -> &str {
    let trimmed = segment.trim_start();
    let end = trimmed
        .find(|character: char| character.is_whitespace() || character == '<' || character == '>')
        .unwrap_or(trimmed.len());
    &trimmed[..end]
}
