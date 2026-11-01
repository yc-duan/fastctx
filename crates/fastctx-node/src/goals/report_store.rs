//! Immutable reports are indexed by conclusion identity and time. The CLI's
//! readable files are disposable projections of these authoritative rows.
use fastctx_types::{
    Address,
    goals::{Report, ReportSummary},
    layout::ConversationDir,
};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Serialize, Deserialize)]
pub(super) struct Frozen {
    pub to: Address,
    pub report: Report,
}

fn directory(conversation: &ConversationDir) -> std::path::PathBuf {
    conversation.books().join("goal-reports")
}

pub(super) fn keep_in(rows: &crate::records::Rows<'_>, proposed: Frozen) -> fastctx_storage::Result<Frozen> {
    if let Some(frozen) = rows.get::<Frozen>("goal-reports", proposed.report.id.as_str())? {
        if frozen.report.id != proposed.report.id
            || frozen.report.goal != proposed.report.goal
            || frozen.report.account != proposed.report.account
        {
            return Err(fastctx_storage::Error::new(super::words::REPORT_CHANGED));
        }
        return Ok(frozen);
    }
    rows.put_ordered("goal-reports", proposed.report.id.as_str(), &proposed, proposed.report.ended_at)?;
    Ok(proposed)
}

pub(crate) fn prepare(conversation: &ConversationDir) -> io::Result<()> {
    let folder = directory(conversation);
    crate::records::ensure_each(conversation, "goal-reports", move |push| {
        let entries = match fastctx_platform::disk::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let bytes = fastctx_platform::disk::read(&path)?;
                let frozen: Frozen = serde_json::from_slice(&bytes)?;
                if path.file_stem().and_then(|stem| stem.to_str()) != Some(frozen.report.id.as_str()) {
                    return Err(crate::record_migration::errors::invalid(
                        "goal report identity differs from its filename",
                    ));
                }
                push(
                    crate::records::Source::new(frozen.report.id.to_string(), path.clone(), bytes)?
                        .ordered(frozen.report.ended_at),
                )
            })?;
        }
        Ok(())
    })
}

pub(crate) fn recent(conversation: &ConversationDir) -> io::Result<Vec<ReportSummary>> {
    prepare(conversation)?;
    let ids = crate::records::read(conversation, |rows| {
        let mut query=rows.connection().prepare_cached("SELECT identity FROM conversation_records WHERE conversation=?1 AND family='goal-reports' ORDER BY arrival DESC,identity DESC LIMIT 10")?;
        Ok(query
            .query_map([rows.conversation()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    let exports = conversation.path().join("goal-reports");
    let mut reports = Vec::new();
    for id in ids.into_iter().rev() {
        let bytes = crate::records::read(conversation, |rows| rows.get_raw("goal-reports", &id))?
            .ok_or_else(|| io::Error::other("goal report disappeared"))?;
        let Frozen { report, .. } = serde_json::from_slice(&bytes)?;
        // The public summary promises a readable artifact. These files can be
        // regenerated from SQL and are never read as authoritative records.
        let path = exports.join(format!("{id}.json"));
        let matches = match fastctx_platform::disk::read(&path) {
            Ok(old) => old == bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if !matches {
            crate::conversation_store::with_live_files(conversation, || {
                fastctx_platform::ensure_private_dir(&exports)?;
                fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
            })?;
        }
        reports.push(ReportSummary {
            id: report.id,
            account: report.account,
            status: report.status,
            ended_at: report.ended_at,
            elapsed_seconds: report.ended_at.as_second().saturating_sub(report.started_at.as_second()).max(0) as u64,
            preview: report.text.lines().next().unwrap_or_default().chars().take(240).collect(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    Ok(reports)
}
