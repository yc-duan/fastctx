//! Recognition of the host's `apply_patch` editing channel being run as a shell program.

const CHANNEL_NAME: &str = "apply_patch";
const MISUSE_NOTE: &str = "(Note: apply_patch is not a program and no shell can run it. Reach it through Codex itself — its own tool call, or Codex's built-in shell — not through this tool.)";

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

#[cfg(test)]
mod tests {
    use super::misuse_note;

    #[test]
    fn failed_command_positions_of_the_channel_name_are_recognized() {
        for command in [
            "apply_patch <<'PATCH'\n*** Begin Patch\n*** End Patch\nPATCH",
            "apply_patch<<'PATCH'\n*** Begin Patch\nPATCH",
            "cd /tmp && apply_patch <<'PATCH'\nPATCH",
            "true; apply_patch 'patch text'",
        ] {
            assert!(misuse_note(command, 127, None).is_some(), "{command}");
        }
    }

    #[test]
    fn a_real_executable_by_that_name_is_never_second_guessed() {
        // Exit 0 means something by that name ran and worked, so the user owns the word here.
        assert!(misuse_note("apply_patch --version", 0, None).is_none());
        // An absolute path names a real program rather than the host channel.
        assert!(misuse_note("/usr/bin/apply_patch foo", 1, None).is_none());
        // Not in command position.
        assert!(misuse_note("echo apply_patch", 1, None).is_none());
        assert!(misuse_note("grep -r apply_patch .", 1, None).is_none());
        // A timeout means the process was alive, so the channel was not what failed.
        assert!(misuse_note("apply_patch <<'PATCH'\nPATCH", 143, Some(500)).is_none());
    }

    #[test]
    fn unrelated_failures_stay_silent() {
        assert!(misuse_note("cargo build", 101, None).is_none());
        assert!(misuse_note("", 127, None).is_none());
    }
}
