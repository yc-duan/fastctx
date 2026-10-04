use std::path::Path;

use crate::source::{Source, splice};

use super::{Plan, files};

pub(super) fn inspect(root: &Path, plan: &mut Plan) {
    let path = root.join("config.toml");
    let Some(snapshot) = plan.read(&path) else { return };
    let Some(bytes) = snapshot.bytes() else { return };
    let location = path.display().to_string();
    let source = match Source::parse(bytes, &location) {
        Ok(source) => source,
        Err(_) => {
            plan.leave(&path, "settings are not valid UTF-8 TOML; preserved without quoting their contents");
            return;
        }
    };
    if source.item(&["schema_version"]).and_then(|item| item.as_integer()) != Some(1) {
        plan.leave(&path, "not the released legacy settings schema; preserved");
        return;
    }
    let version = source
        .item(&["last_seen_version"])
        .and_then(|item| item.as_str())
        .or_else(|| source.item(&["applied", "version"]).and_then(|item| item.as_str()));
    let canonical_shell = ["enabled", "job_storage_limit_mib", "max_running_jobs", "job_list_limit"]
        .iter()
        .all(|key| source.item(&["fastshell", key]).is_some());
    if !version.is_some_and(files::released) && !canonical_shell {
        plan.leave(&path, "customized or unknown settings have no original-content receipt; preserved");
        return;
    }
    if version.is_some_and(|version| !files::released(version)) {
        plan.leave(&path, "settings name a version outside 0.1.0 through 0.2.6; preserved");
        return;
    }
    if source.item(&["applied"]).is_none() && !files::plain(&snapshot) {
        if plan.quiet(&path, false) && plan.unused(&path, true) {
            plan.archive(&path, "settings", "legacy settings with named data retained unchanged");
        }
        return;
    }
    let mut defaults = vec![
        (vec!["tier"], "\"standard\"".to_string()),
        (vec!["fastshell", "enabled"], "false".into()),
        (vec!["fastshell", "job_storage_limit_mib"], "1024".into()),
        (vec!["fastshell", "max_running_jobs"], "128".into()),
        (vec!["fastshell", "job_list_limit"], "20".into()),
        (vec!["update", "auto_check"], "true".into()),
        (vec!["update", "source"], "\"auto\"".into()),
        (vec!["tool_budget_epoch"], "2".into()),
    ];
    // Before 0.2.2 all five shares were serialized defaults. From 0.2.3
    // onward presence represents an explicit preference, even at a default value.
    if matches!(version, Some("0.1.0" | "0.1.1" | "0.2.0" | "0.2.1")) || version.is_none() {
        for (key, value) in [
            ("read", "inherit"),
            ("grep", "inherit"),
            ("glob", "percent50"),
            ("run", "inherit"),
            ("job_output", "inherit"),
        ] {
            defaults.push((vec!["tool_budgets", key], format!("\"{value}\"")));
        }
    }
    let mut edits = Vec::new();
    for (keys, value) in defaults {
        if source.item(&keys).is_none() {
            continue;
        }
        let Ok(range) = source.assignment(&keys) else { continue };
        let expected = format!("{} = {value}\n", keys.last().expect("nonempty path"));
        let start = range.start;
        if (start == 0 || bytes[start - 1] == b'\n')
            && bytes.get(start..start + expected.len()) == Some(expected.as_bytes())
        {
            edits.push((start..start + expected.len(), Vec::new()));
        }
    }
    let mut next = match splice(bytes, edits) {
        Ok(next) => next,
        Err(error) => {
            plan.leave(&path, error.to_string());
            return;
        }
    };
    for table in ["fastshell", "update", "tool_budgets"] {
        let parsed = match Source::parse(&next, &location) {
            Ok(parsed) => parsed,
            Err(error) => {
                plan.leave(&path, error.to_string());
                return;
            }
        };
        if let Some(section) = parsed.item(&[table]).and_then(|item| item.as_table()).filter(|table| table.is_empty()) {
            let header = format!("[{table}]\n");
            if let Some(start) = section.span().map(|range| range.start + parsed.root_offset())
                && (start == 0 || next[start - 1] == b'\n')
                && next.get(start..start + header.len()) == Some(header.as_bytes())
            {
                next.drain(start..start + header.len());
            }
        }
    }
    let parsed = match Source::parse(&next, &location) {
        Ok(parsed) => parsed,
        Err(error) => {
            plan.leave(&path, error.to_string());
            return;
        }
    };
    let user_settings =
        parsed.doc.as_table().iter().any(|(key, _)| !["schema_version", "applied", "last_seen_version"].contains(&key));
    // Keep the version watermark with retained preferences so subsequent
    // starts can still attribute the remaining legacy settings.
    if !user_settings
        && let Some(version) = version
        && let Ok(range) = parsed.assignment(&["last_seen_version"])
    {
        let expected = format!("last_seen_version = \"{version}\"\n");
        if next.get(range.start..range.start + expected.len()) == Some(expected.as_bytes()) {
            next.drain(range.start..range.start + expected.len());
        }
    }
    let parsed = match Source::parse(&next, &location) {
        Ok(parsed) => parsed,
        Err(error) => {
            plan.leave(&path, error.to_string());
            return;
        }
    };
    let only_schema = parsed.doc.as_table().len() == 1;
    if only_schema && next.starts_with(b"schema_version = 1\n") {
        next.drain(.."schema_version = 1\n".len());
    }
    let parsed = match Source::parse(&next, &location) {
        Ok(parsed) => parsed,
        Err(error) => {
            plan.leave(&path, error.to_string());
            return;
        }
    };
    if parsed.doc.as_table().iter().any(|(key, _)| !["schema_version", "applied"].contains(&key)) {
        if source.item(&["applied"]).is_none() {
            if plan.quiet(&path, false) && plan.unused(&path, true) {
                plan.archive(&path, "settings", "legacy preferences and user edits retained unchanged");
            }
            return;
        }
        plan.leave(
            &path,
            "custom settings remain beside the applied receipt; they will be archived after apply retires that receipt",
        );
    }
    if source.item(&["applied"]).is_none() && !next.iter().all(u8::is_ascii_whitespace) {
        if plan.quiet(&path, false) && plan.unused(&path, true) {
            plan.archive(&path, "settings", "legacy settings with user text retained unchanged");
        }
        return;
    }
    if next != bytes {
        let after = if next.iter().all(u8::is_ascii_whitespace) { None } else { Some(next) };
        plan.edits.push((
            snapshot,
            after,
            "retire exact serialized legacy defaults; preserve receipt and user bytes".into(),
        ));
    }
}
