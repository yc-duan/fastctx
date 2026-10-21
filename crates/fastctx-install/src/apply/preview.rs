//! What an apply or unapply of one login would change, worked out from the
//! files as they are now by the same planners the change uses, with nothing
//! written (`world-gui.md`: the change's card shows it before the user
//! agrees). For Claude Code, its own plugin commands are not run; which of
//! them would run follows from its plugin listings, which are read.

use fastctx_types::AgentName;
use fastctx_types::logins::{ChangePreview, FilePreview, Kept, OutputLimit};

use super::*;

mod items;

/// The preview of putting the World's tools into `host`'s selected home
/// (`into`), doing `edits` with what the user changed of the World's, or
/// taking them out.
pub fn preview(places: &Places, host: Host, into: bool, edits: Edits) -> Result<ChangePreview, Error> {
    let record_file = Snapshot::read(places.record())?;
    let records = homes::Records::read(&record_file, places)?;
    let record = records.selected(places, host.only())?;
    let (agent, dir) = match host {
        Host::Codex => ("codex", places.codex_home.clone()),
        Host::Claude => ("claude", places.claude_home.clone()),
    };
    let mut preview = ChangePreview {
        agent: AgentName::try_from(agent).expect("an agent name"),
        dir,
        into,
        files: Vec::new(),
        commands: Vec::new(),
        output_limit: None,
        kept: Vec::new(),
    };
    match host {
        Host::Codex => codex(places, &record, into, edits, &mut preview)?,
        Host::Claude => claude(places, &record, into, edits, &mut preview)?,
    }
    Ok(preview)
}

fn codex(places: &Places, record: &Record, into: bool, edits: Edits, preview: &mut ChangePreview) -> Result<(), Error> {
    let config = Snapshot::read(places.codex_home.join("config.toml"))?;
    let agents = Snapshot::read(places.codex_home.join("AGENTS.md"))?;
    let legacy = if record.codex.is_none() { homes::legacy(places)? } else { None };
    let (old_file, mut old) = legacy.map(|(file, receipt)| (Some(file), Some(receipt))).unwrap_or_default();
    if let Some(old) = &mut old {
        bind_legacy(old, &config, &agents)?;
    }
    let (config_path, agents_path) = (text(config.path()), text(agents.path()));
    let config_file = File { path: &config_path, bytes: config.bytes() };
    let agents_file = File { path: &agents_path, bytes: agents.bytes() };
    let (config_after, agents_after, notices) = if into {
        let ownership = match (&record.codex, &old) {
            (Some(receipt), _) if edits == Edits::TakeBack => Ownership::TakeBack(receipt),
            (Some(receipt), _) => Ownership::Current(receipt),
            (None, Some(receipt)) => Ownership::Legacy(receipt),
            (None, None) => Ownership::None,
        };
        let plan = CodexPlan::new(&places.installed());
        let guidance = briefing::guidance(true);
        let planned = |approval| codex::apply(config_file, agents_file, &plan, &guidance, ownership, approval);
        let applied = match planned(Approval::Require) {
            Err(crate::Error::TokenLimitApproval { location, current, requested }) => {
                preview.output_limit = Some(OutputLimit { location, current, requested });
                planned(Approval::RaiseOutputLimit)?
            }
            other => other?,
        };
        (Some(applied.config), Some(applied.agents), applied.notices)
    } else if record.codex.is_some() || old.is_some() {
        let taken = match &old {
            Some(old) => codex::unapply_legacy(config_file, agents_file, old)?,
            None => codex::unapply(config_file, agents_file, record.codex.as_ref())?,
        };
        (taken.config, taken.agents, taken.retained)
    } else {
        (config.bytes().map(<[u8]>::to_vec), agents.bytes().map(<[u8]>::to_vec), Vec::new())
    };
    file(preview, config.path(), config.bytes(), config_after.as_deref(), Kind::Toml(items::codex_keys))?;
    file(preview, agents.path(), agents.bytes(), agents_after.as_deref(), Kind::Guidance)?;
    if let Some(old_file) = old_file {
        let path = text(old_file.path());
        let retired = legacy::retire(File { path: &path, bytes: old_file.bytes() })?;
        file(preview, old_file.path(), old_file.bytes(), Some(&retired), Kind::Toml(|_| false))?;
    }
    preview.kept.extend(notices.into_iter().map(kept));
    Ok(())
}

fn claude(
    places: &Places,
    record: &Record,
    into: bool,
    edits: Edits,
    preview: &mut ChangePreview,
) -> Result<(), Error> {
    let settings_file = Snapshot::read(places.claude_home.join("settings.json"))?;
    let claude_md = Snapshot::read(places.claude_home.join("CLAUDE.md"))?;
    let settings_path = text(settings_file.path());
    let md_path = text(claude_md.path());
    let plan = ClaudePlan::new(&places.world);
    let plugin_record = homes::plugin_record(places)?;
    let previous = read_plugin_record(&plugin_record, settings_file.path(), record)?;
    let listings = |claude: PathBuf| -> Result<plugins::PluginState, Error> {
        let settings_created = record.settings_created || previous.as_ref().is_some_and(|r| r.settings_created);
        let mut host = Cli::new(claude, plugin_record.clone(), settings_file.path().to_owned(), settings_created);
        plugins::snapshot(&mut host).map_err(Error::Claude)
    };
    if into {
        let claude = places.claude.clone().ok_or(Error::NoClaude)?;
        let state = listings(claude)?;
        let planned =
            PluginApply::prepare(&plan, state, previous.as_ref().map(|r| &r.receipt)).map_err(Error::Claude)?;
        preview.commands =
            planned.commands().iter().map(|command| format!("claude {}", command.args.join(" "))).collect();
        let edit = settings::apply(settings_file.bytes(), &plan, record.settings.as_ref(), edits == Edits::TakeBack).map_err(Error::Claude)?;
        kept_limit(preview, settings_file.path(), Limit::output(&plan).key, &edit);
        let edit = settings::apply_limit(edit.bytes.as_deref(), Limit::description(&plan), record.description.as_ref(), edits == Edits::TakeBack)
            .map_err(Error::Claude)?;
        kept_limit(preview, settings_file.path(), Limit::description(&plan).key, &edit);
        let tools = crate::claude::tools::apply(
            File { path: &settings_path, bytes: edit.bytes.as_deref() },
            record.claude_tools.as_ref(),
            edits == Edits::TakeBack,
        )
        .map_err(Error::Claude)?;
        preview.kept.extend(tools.notices.into_iter().map(kept));
        let (after, _) =
            crate::claude::consent::grant(tools.bytes.as_deref(), record.consent.as_ref()).map_err(Error::Claude)?;
        file(preview, settings_file.path(), settings_file.bytes(), after.as_deref(), Kind::Json(items::claude_keys))?;
        let guide = if edits == Edits::TakeBack { guidance::take_back } else { guidance::apply };
        let guidance = guide(
            File { path: &md_path, bytes: claude_md.bytes() },
            &briefing::guidance(false),
            record.claude_md.as_ref(),
        )?;
        file(preview, claude_md.path(), claude_md.bytes(), Some(&guidance.bytes), Kind::Guidance)?;
        return Ok(());
    }
    match places.claude.clone() {
        Some(claude) => {
            let state = listings(claude)?;
            preview.commands = plugins::withdrawal(&plan, &state)
                .map_err(Error::Claude)?
                .iter()
                .map(|command| format!("claude {}", command.args.join(" ")))
                .collect();
        }
        None if previous.as_ref().is_some_and(|record| record.receipt.has_work()) => return Err(Error::NoClaude),
        None => {}
    }
    let mut bytes =
        crate::claude::registration::remove(settings_file.bytes(), &plan, true, true).map_err(Error::Claude)?;
    if let Some(receipt) = &record.claude_tools {
        let edit = crate::claude::tools::unapply(File { path: &settings_path, bytes: bytes.as_deref() }, receipt)
            .map_err(Error::Claude)?;
        preview.kept.extend(edit.notices.into_iter().map(kept));
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.settings {
        let edit = settings::unapply(bytes.as_deref(), Some(receipt)).map_err(Error::Claude)?;
        kept_limit(preview, settings_file.path(), Limit::output(&plan).key, &edit);
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.description {
        let key = Limit::description(&plan).key;
        let edit = settings::unapply_limit(bytes.as_deref(), key, Some(receipt)).map_err(Error::Claude)?;
        kept_limit(preview, settings_file.path(), key, &edit);
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.consent {
        let (restored, left) = crate::claude::consent::restore(bytes.as_deref(), receipt).map_err(Error::Claude)?;
        if left {
            preview.kept.push(kept(consent_kept(settings_file.path())));
        }
        bytes = restored;
    }
    if record.settings_created || previous.as_ref().is_some_and(|record| record.settings_created) {
        bytes = settings::discard_empty_created(bytes).map_err(Error::Claude)?;
    }
    file(preview, settings_file.path(), settings_file.bytes(), bytes.as_deref(), Kind::Json(items::claude_keys))?;
    if let Some(receipt) = &record.claude_md {
        let taken = guidance::unapply(File { path: &md_path, bytes: claude_md.bytes() }, Some(receipt))?;
        preview.kept.extend(taken.retained.into_iter().map(kept));
        file(preview, claude_md.path(), claude_md.bytes(), taken.bytes.as_deref(), Kind::Guidance)?;
    }
    Ok(())
}

enum Kind {
    Toml(items::Shown),
    Json(items::Shown),
    Guidance,
}

/// Adds `path` to the preview when its bytes would change.
fn file(
    preview: &mut ChangePreview,
    path: &Path,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
    kind: Kind,
) -> Result<(), Error> {
    if before == after {
        return Ok(());
    }
    let changed = match kind {
        Kind::Toml(shown) => items::toml(before, after, shown),
        Kind::Json(shown) => items::json(before, after, shown),
        Kind::Guidance => Ok(items::guidance(before, after)),
    }
    .map_err(|reason| Error::Io(format!("{}: {reason}", path.display())))?;
    preview.files.push(FilePreview {
        path: path.to_path_buf(),
        how: items::how(before.is_some(), after.is_some()),
        items: changed,
    });
    Ok(())
}

/// A limit the user changed since the apply, which is left as they set it.
fn kept_limit(preview: &mut ChangePreview, path: &Path, key: &str, edit: &settings::SettingsEdit) {
    if edit.outcome == settings::SettingsOutcome::UserChanged {
        preview.kept.push(Kept {
            location: format!("{}: env.{key}", path.display()),
            reason: "changed since apply; left untouched".into(),
        });
    }
}

fn kept(notice: Notice) -> Kept {
    Kept { location: notice.location, reason: notice.reason }
}
