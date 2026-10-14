//! Keeping the World's tools in a login as this fastctx writes them
//! (`world-agent-logins.md`, 接入跟着版本走). What an apply would change in a
//! login they are in is worked out first, with nothing written; when that
//! touches only what the World put in, as it put it in, the apply is made.
//! What the user changed of it since, a limit only they may raise, and a
//! plugin taken out of Claude Code stay as they are, said for the user.

use fastctx_types::logins::OutputLimit;

use super::*;

/// What keeping one login up to date came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The World's tools are not in it.
    Out,
    /// They are as this fastctx writes them.
    Current,
    /// They were brought up to date: the files written.
    Refreshed(Vec<PathBuf>),
    /// Something of theirs was changed since, and is left as it is.
    Changed { location: String, reason: String },
    /// Bringing them up to date raises Codex's shared output limit, which
    /// waits for the user's word.
    Asks(OutputLimit),
}

/// Brings the World's tools in `host`'s selected home up to what this
/// fastctx writes, where nothing of that needs the user.
pub fn refresh(places: &Places, host: Host) -> Result<Refresh, Error> {
    let home = match host {
        Host::Codex => &places.codex_home,
        Host::Claude => &places.claude_home,
    };
    if status_at(places, host, home)? != Standing::In {
        return Ok(Refresh::Out);
    }
    let preview = match preview(places, host, true, Edits::Keep) {
        Ok(preview) => preview,
        Err(Error::Edit(crate::Error::Conflict { location, reason })) => {
            return Ok(Refresh::Changed { location, reason });
        }
        Err(error) => return Err(error),
    };
    if let Some(limit) = preview.output_limit {
        return Ok(Refresh::Asks(limit));
    }
    // A plugin the World put into Claude Code that is no longer there, or no
    // longer on, was taken out by the user or by Claude Code itself.
    if let Some(command) = preview.commands.first() {
        return Ok(Refresh::Changed {
            location: places.claude_home.display().to_string(),
            reason: format!("the World's plugin is not in Claude Code as it was put in; `{command}` puts it back"),
        });
    }
    if preview.files.is_empty() {
        return Ok(Refresh::Current);
    }
    apply_reporting(places, host.only(), Approval::Require, Edits::Keep, None)?;
    Ok(Refresh::Refreshed(preview.files.into_iter().map(|file| file.path).collect()))
}
