//! Keeping the World's tools in a login as this fastctx writes them
//! (`world-agent-logins.md`, 接入跟着版本走), by itself and without asking:
//! what the World put in is written as this version writes it, whoever
//! changed it since, the user, another program or the host itself; a
//! plugin taken out or turned off is put back, and its record brought to
//! this version; Codex's shared output limit is raised where it was set
//! lower. Before anything someone else changed is taken back, the files as
//! they were are kept aside. What cannot be done by itself is said.

use std::path::PathBuf;

use super::*;

/// What keeping one login up to date came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The World's tools are not in it.
    Out,
    /// They are as this fastctx writes them.
    Current,
    /// They were brought up to date: the files written, the plugin commands
    /// run, and where the files were kept as they were when something
    /// someone else changed was taken back.
    Refreshed { files: Vec<PathBuf>, commands: Vec<String>, kept: Option<PathBuf> },
    /// They could not be brought up to date by themselves: where, and why.
    Changed { location: String, reason: String },
}

/// Brings the World's tools in `host`'s selected home up to what this
/// fastctx writes.
pub fn refresh(places: &Places, host: Host) -> Result<Refresh, Error> {
    let home = match host {
        Host::Codex => &places.codex_home,
        Host::Claude => &places.claude_home,
    };
    if status_at(places, host, home)? != Standing::In {
        return Ok(Refresh::Out);
    }
    // What keeping everyone else's changes would leave says whether any is
    // taken back.
    let taken = match preview(places, host, true, Edits::Keep) {
        Ok(kept) => !kept.kept.is_empty() || kept.output_limit.is_some(),
        Err(Error::Edit(crate::Error::Conflict { .. })) => true,
        Err(error) => return Err(error),
    };
    let preview = match preview(places, host, true, Edits::TakeBack) {
        Ok(preview) => preview,
        Err(Error::Edit(crate::Error::Conflict { location, reason })) => {
            return Ok(Refresh::Changed { location, reason });
        }
        Err(error) => return Err(error),
    };
    if preview.files.is_empty() && preview.commands.is_empty() {
        return Ok(Refresh::Current);
    }
    let files: Vec<PathBuf> = preview.files.iter().map(|file| file.path.clone()).collect();
    let kept = if taken { Some(keep(places, host, &files)?) } else { None };
    apply_reporting(places, host.only(), Approval::RaiseOutputLimit, Edits::TakeBack, None)?;
    Ok(Refresh::Refreshed { files, commands: preview.commands, kept })
}

/// Copies of `files` as they are now, in a folder of their own under
/// fastctx's directory named for the moment and the agent.
fn keep(places: &Places, host: Host, files: &[PathBuf]) -> Result<PathBuf, Error> {
    let agent = match host {
        Host::Codex => "codex",
        Host::Claude => "claude",
    };
    let folder = places.root.join("kept").join(format!("{}-{agent}", jiff::Timestamp::now().strftime("%Y-%m-%d-%H%M%S")));
    std::fs::create_dir_all(&folder).map_err(io("keeping the files taken back"))?;
    for file in files {
        let Some(name) = file.file_name() else { continue };
        match std::fs::copy(file, folder.join(name)) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io("keeping the files taken back")(error)),
        }
    }
    Ok(folder)
}
