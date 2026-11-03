//! What an apply or unapply of one login would change, worked out from the
//! files as they are now by the same planners the change uses, with nothing
//! written (`world-layers.md`, 接入之前讲清楚做了什么: the window before the
//! change shows it, and the user agrees to it). For Claude Code, its own
//! plugin commands are not run; which of them would run follows from its
//! plugin listings, which are read.
//!
//! The preview is told twice: item by item, as an older interface shows it,
//! and as one card each kind of change with the lines it adds and takes out
//! (`crate::connect`). Its fingerprint is that of the files it was worked
//! out from, for the change to be refused when they moved on since.

use fastctx_types::AgentName;
use fastctx_types::logins::{Change, ChangePreview, FilePreview, Kept, OutputLimit};

use super::*;
use crate::connect::Steps;

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
        cards: Vec::new(),
        fingerprint: None,
    };
    let mut steps = Steps::default();
    match host {
        Host::Codex => codex(places, &record, into, edits, &mut preview, &mut steps)?,
        Host::Claude => claude(places, &record, into, edits, &mut preview, &mut steps)?,
    }
    preview.cards = steps.cards();
    preview.fingerprint = Some(fingerprint(places, host)?);
    Ok(preview)
}

/// The fingerprint of what a change of `host`'s selected home is worked out
/// from: the record of earlier changes and each file its planners read. Of
/// Claude Code's global state, which it rewrites with every session, only
/// Fastctx's entry counts.
pub fn fingerprint(places: &Places, host: Host) -> Result<String, Error> {
    let mut files = vec![Snapshot::read(places.record())?];
    let mut state = None;
    match host {
        Host::Codex => {
            files.push(Snapshot::read(places.codex_home.join("config.toml"))?);
            files.push(Snapshot::read(places.codex_home.join("AGENTS.md"))?);
            if let Some((legacy, _)) = homes::legacy(places)? {
                files.push(legacy);
            }
        }
        Host::Claude => {
            files.push(Snapshot::read(places.claude_home.join("settings.json"))?);
            files.push(Snapshot::read(places.claude_home.join("CLAUDE.md"))?);
            files.push(Snapshot::read(homes::plugin_record(places)?)?);
            files.push(Snapshot::read(homes::entry_record(places)?)?);
            state = Some(state::print(places)?);
        }
    }
    let mut inputs: Vec<_> = files.iter().map(|file| (file.path(), file.bytes())).collect();
    if let Some((path, entry)) = &state {
        inputs.push((path.as_path(), entry.as_deref()));
    }
    Ok(crate::connect::fingerprint(&inputs))
}

fn codex(
    places: &Places,
    record: &Record,
    into: bool,
    edits: Edits,
    preview: &mut ChangePreview,
    steps: &mut Steps,
) -> Result<(), Error> {
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
    let (config_after, agents_after, notices, made) = if into {
        let ownership = match (&record.codex, &old) {
            (Some(receipt), _) if edits == Edits::TakeBack => Ownership::TakeBack(receipt),
            (Some(receipt), _) => Ownership::Current(receipt),
            (None, Some(receipt)) => Ownership::Legacy(receipt),
            (None, None) => Ownership::None,
        };
        let plan = codex_plan(places, &places.installed(), record.codex.as_ref());
        let guidance = briefing::guidance(true);
        let planned = |approval| codex::apply(config_file, agents_file, &plan, &guidance, ownership, approval);
        let applied = match planned(Approval::Require) {
            Err(crate::Error::TokenLimitApproval { location, current, requested }) => {
                preview.output_limit = Some(OutputLimit { location, current, requested });
                planned(Approval::RaiseOutputLimit)?
            }
            other => other?,
        };
        (Some(applied.config), Some(applied.agents), applied.notices, applied.steps)
    } else if record.codex.is_some() || old.is_some() {
        let taken = match &old {
            Some(old) => codex::unapply_legacy(config_file, agents_file, old)?,
            None => codex::unapply(config_file, agents_file, record.codex.as_ref())?,
        };
        (taken.config, taken.agents, taken.retained, taken.steps)
    } else {
        (config.bytes().map(<[u8]>::to_vec), agents.bytes().map(<[u8]>::to_vec), Vec::new(), Steps::default())
    };
    steps.extend(made);
    file(preview, config.path(), config.bytes(), config_after.as_deref(), Kind::Toml(items::codex_keys))?;
    file(preview, agents.path(), agents.bytes(), agents_after.as_deref(), Kind::Guidance)?;
    if let Some(old_file) = old_file {
        let path = text(old_file.path());
        let retired = legacy::retire(File { path: &path, bytes: old_file.bytes() })?;
        steps.record(Change::Legacy, old_file.path(), old_file.bytes(), Some(&retired));
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
    steps: &mut Steps,
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
    let settings_at = settings_file.path();
    let (_, entry_receipt) = state::receipt(places)?;
    if into {
        let workbench = claude_workbench(places, record);
        let withdrawal = |claude: PathBuf| -> Result<Vec<String>, Error> {
            Ok(plugins::withdrawal(&plan, &listings(claude)?)
                .map_err(Error::Claude)?
                .iter()
                .map(|command| format!("claude {}", command.args.join(" ")))
                .collect())
        };
        if workbench {
            let claude = places.claude.clone().ok_or(Error::NoClaude)?;
            let state = listings(claude)?;
            let planned =
                PluginApply::prepare(&plan, state, previous.as_ref().map(|r| &r.receipt)).map_err(Error::Claude)?;
            preview.commands =
                planned.commands().iter().map(|command| format!("claude {}", command.args.join(" "))).collect();
        } else if previous.as_ref().is_some_and(|record| record.receipt.has_work())
            && let Some(claude) = places.claude.clone()
        {
            // Connected alone, a login keeps no plugin an earlier change put in.
            preview.commands = withdrawal(claude)?;
        }
        for line in &preview.commands {
            steps.command(Change::Plugin, line.clone());
        }
        let take_back = edits == Edits::TakeBack;
        let start = if workbench {
            settings_file.bytes().map(<[u8]>::to_vec)
        } else {
            crate::claude::registration::remove(settings_file.bytes(), &plan, true, true).map_err(Error::Claude)?
        };
        steps.record(Change::Plugin, settings_at, settings_file.bytes(), start.as_deref());
        let output =
            settings::apply(start.as_deref(), &plan, record.settings.as_ref(), take_back).map_err(Error::Claude)?;
        steps.record(Change::Limits, settings_at, start.as_deref(), output.bytes.as_deref());
        kept_limit(preview, settings_at, Limit::output(&plan).key, &output);
        let described = settings::apply_limit(
            output.bytes.as_deref(),
            Limit::description(&plan),
            record.description.as_ref(),
            take_back,
        )
        .map_err(Error::Claude)?;
        steps.record(Change::Limits, settings_at, output.bytes.as_deref(), described.bytes.as_deref());
        kept_limit(preview, settings_at, Limit::description(&plan).key, &described);
        let tools = crate::claude::tools::apply(
            File { path: &settings_path, bytes: described.bytes.as_deref() },
            record.claude_tools.as_ref(),
            take_back,
        )
        .map_err(Error::Claude)?;
        steps.record(Change::Shell, settings_at, described.bytes.as_deref(), tools.bytes.as_deref());
        preview.kept.extend(tools.notices.into_iter().map(kept));
        let after = if workbench {
            crate::claude::consent::grant(tools.bytes.as_deref(), record.consent.as_ref()).map_err(Error::Claude)?.0
        } else if let Some(receipt) = &record.consent {
            let (restored, left) =
                crate::claude::consent::restore(tools.bytes.as_deref(), receipt).map_err(Error::Claude)?;
            if left {
                preview.kept.push(kept(consent_kept(settings_at)));
            }
            restored
        } else {
            tools.bytes.clone()
        };
        steps.record(Change::Consent, settings_at, tools.bytes.as_deref(), after.as_deref());
        file(preview, settings_at, settings_file.bytes(), after.as_deref(), Kind::Json(items::claude_keys))?;
        let guide = if take_back { guidance::take_back } else { guidance::apply };
        let guidance = guide(
            File { path: &md_path, bytes: claude_md.bytes() },
            &briefing::guidance(false),
            record.claude_md.as_ref(),
        )?;
        steps.record(Change::Guidance, claude_md.path(), claude_md.bytes(), Some(&guidance.bytes));
        file(preview, claude_md.path(), claude_md.bytes(), Some(&guidance.bytes), Kind::Guidance)?;
        // The World's server: from the plugin with the workbench, and
        // otherwise entered in Claude Code's global state.
        let planned = if workbench {
            entry_receipt.as_ref().map(state::plan_take).transpose()?
        } else {
            Some(state::plan_put(places, entry_receipt.as_ref(), take_back)?)
        };
        if let Some(planned) = planned {
            entry_file(preview, steps, &planned)?;
        }
        return Ok(());
    }
    // A login that never had the plugin starts none of Claude Code's
    // programs to be taken out.
    match places.claude.clone().filter(|_| previous.is_some()) {
        Some(claude) => {
            let state = listings(claude)?;
            preview.commands = plugins::withdrawal(&plan, &state)
                .map_err(Error::Claude)?
                .iter()
                .map(|command| format!("claude {}", command.args.join(" ")))
                .collect();
            for line in &preview.commands {
                steps.command(Change::Plugin, line.clone());
            }
        }
        None if previous.as_ref().is_some_and(|record| record.receipt.has_work()) => return Err(Error::NoClaude),
        None => {}
    }
    let mut bytes =
        crate::claude::registration::remove(settings_file.bytes(), &plan, true, true).map_err(Error::Claude)?;
    steps.record(Change::Plugin, settings_at, settings_file.bytes(), bytes.as_deref());
    if let Some(receipt) = &record.claude_tools {
        let edit = crate::claude::tools::unapply(File { path: &settings_path, bytes: bytes.as_deref() }, receipt)
            .map_err(Error::Claude)?;
        preview.kept.extend(edit.notices.into_iter().map(kept));
        steps.record(Change::Shell, settings_at, bytes.as_deref(), edit.bytes.as_deref());
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.description {
        let key = Limit::description(&plan).key;
        let edit = settings::unapply_limit(bytes.as_deref(), key, Some(receipt)).map_err(Error::Claude)?;
        kept_limit(preview, settings_at, key, &edit);
        steps.record(Change::Limits, settings_at, bytes.as_deref(), edit.bytes.as_deref());
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.settings {
        let edit = settings::unapply(bytes.as_deref(), Some(receipt)).map_err(Error::Claude)?;
        kept_limit(preview, settings_at, Limit::output(&plan).key, &edit);
        steps.record(Change::Limits, settings_at, bytes.as_deref(), edit.bytes.as_deref());
        bytes = edit.bytes;
    }
    if let Some(receipt) = &record.consent {
        let (restored, left) = crate::claude::consent::restore(bytes.as_deref(), receipt).map_err(Error::Claude)?;
        if left {
            preview.kept.push(kept(consent_kept(settings_at)));
        }
        steps.record(Change::Consent, settings_at, bytes.as_deref(), restored.as_deref());
        bytes = restored;
    }
    if record.settings_created || previous.as_ref().is_some_and(|record| record.settings_created) {
        let discarded = settings::discard_empty_created(bytes.clone()).map_err(Error::Claude)?;
        steps.record(Change::Limits, settings_at, bytes.as_deref(), discarded.as_deref());
        bytes = discarded;
    }
    file(preview, settings_at, settings_file.bytes(), bytes.as_deref(), Kind::Json(items::claude_keys))?;
    if let Some(receipt) = &record.claude_md {
        let taken = guidance::unapply(File { path: &md_path, bytes: claude_md.bytes() }, Some(receipt))?;
        preview.kept.extend(taken.retained.into_iter().map(kept));
        steps.record(Change::Guidance, claude_md.path(), claude_md.bytes(), taken.bytes.as_deref());
        file(preview, claude_md.path(), claude_md.bytes(), taken.bytes.as_deref(), Kind::Guidance)?;
    }
    if let Some(receipt) = &entry_receipt {
        entry_file(preview, steps, &state::plan_take(receipt)?)?;
    }
    Ok(())
}

/// Fastctx's entry in Claude Code's global state, put in or taken out: told
/// on the tools' card with its own lines alone, and by its own keys.
fn entry_file(preview: &mut ChangePreview, steps: &mut Steps, planned: &state::Planned) -> Result<(), Error> {
    let (before, after) = (planned.before.as_deref(), planned.after.as_deref());
    steps.record_bare(Change::Tools, &planned.path, before, after);
    file(preview, &planned.path, before, after, Kind::Json(crate::claude::entry::shown))
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
