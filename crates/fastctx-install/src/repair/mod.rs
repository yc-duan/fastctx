//! Repair uses the same publication, process identities and service registrations
//! as renewal. It never deletes lock files or edits the World's user data.

#[cfg(target_os = "linux")]
mod appimage;
pub mod application;
pub mod package;
pub mod records;
pub mod target;

use anyhow::Context as _;
use fastctx_types::repair::{Outcome, Report, Step};
use std::path::Path;

pub fn read(root: &Path) -> std::io::Result<Option<Report>> {
    crate::renewal::read_json(&root.join("updates/repair.json"))
}

pub struct Journal {
    root: std::path::PathBuf,
    pub report: Report,
}

impl Journal {
    pub fn start(root: &Path, version: &str, by: &str, id: Option<String>) -> anyhow::Result<Self> {
        let started = jiff::Timestamp::now();
        let process = fastctx_platform::current_identity()?;
        let mut inherited = Vec::new();
        if let Some(id) = &id
            && let Some(previous) = read(root)?
            && previous.id == *id
            && previous.finished.is_none()
        {
            // Only the package step is handed over unfinished: the earlier
            // program obtained and checked the package, and this one goes on
            // from it. Any other step left checking was cut off, and is
            // checked again from the start.
            inherited = previous.steps;
            inherited.retain(|step| step.outcome != Outcome::Checking || step.layer == "package");
            for step in &mut inherited {
                if step.outcome == Outcome::Checking {
                    step.outcome = if step.actions.is_empty() { Outcome::Unchanged } else { Outcome::Repaired };
                    step.detail = "verified acquisition handed to this release's repair program".into();
                }
            }
        }
        let journal = Self {
            root: root.to_owned(),
            report: Report {
                id: id.unwrap_or_else(|| format!("{}-{}", started.as_millisecond(), process.pid)),
                version: version.into(),
                by: by.into(),
                started,
                finished: None,
                process,
                steps: inherited,
                success: false,
                unchanged: false,
                error: None,
            },
        };
        journal.save()?;
        Ok(journal)
    }

    /// Transfers an accepted background repair to its identified child before
    /// releasing the exclusion gate that child is waiting for.
    pub fn handoff(&mut self, process: fastctx_types::ProcessIdentity) -> anyhow::Result<()> {
        self.report.process = process;
        self.save()
    }

    pub fn set_version(&mut self, version: String) -> anyhow::Result<()> {
        self.report.version = version;
        self.save()
    }

    pub fn begin(&mut self, layer: &str) -> anyhow::Result<()> {
        if let Some(step) = self.report.steps.last_mut()
            && step.layer == layer
            && matches!(step.outcome, Outcome::Unchanged | Outcome::Repaired)
        {
            step.outcome = Outcome::Checking;
            step.detail.clear();
            return self.save();
        }
        self.report.steps.push(Step {
            layer: layer.into(),
            outcome: Outcome::Checking,
            detail: String::new(),
            actions: Vec::new(),
        });
        self.save()
    }

    /// Records the intended mutation before it is attempted.
    pub fn action(&mut self, description: String) -> anyhow::Result<()> {
        if let Some(step) = self.report.steps.last_mut() {
            step.actions.push(description);
        }
        self.save()
    }

    pub fn end(&mut self, result: anyhow::Result<String>) -> anyhow::Result<bool> {
        let step = self.report.steps.last_mut().context("repair has no active layer")?;
        let success = result.is_ok();
        match result {
            Ok(detail) => {
                step.outcome = if step.actions.is_empty() { Outcome::Unchanged } else { Outcome::Repaired };
                step.detail = detail;
            }
            Err(error) => {
                step.outcome = Outcome::Failed;
                step.detail = format!("{error:#}");
            }
        }
        eprintln!("repair {}: {:?}: {}", step.layer, step.outcome, step.detail);
        self.save()?;
        Ok(success)
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        self.report.finished = Some(jiff::Timestamp::now());
        self.report.success =
            self.report.steps.iter().all(|s| matches!(s.outcome, Outcome::Unchanged | Outcome::Repaired));
        self.report.unchanged =
            self.report.success && self.report.steps.iter().all(|s| s.outcome == Outcome::Unchanged);
        self.report.error = (!self.report.success).then(|| {
            self.report
                .steps
                .iter()
                .filter(|s| s.outcome == Outcome::Failed)
                .map(|s| format!("{}: {}", s.layer, s.detail))
                .collect::<Vec<_>>()
                .join("; ")
        });
        self.save()
    }

    fn save(&self) -> anyhow::Result<()> {
        let path = self.root.join("updates/repair.json");
        crate::renewal::write_json(&path, &self.report)
            .with_context(|| format!("saving repair record {}", path.display()))
    }
}

/// Finalizes an error outside a layer (including a failed bootstrap). A newer
/// repair or a child repairer's receipt is never overwritten.
pub fn record_failure(root: &Path, id: Option<&str>, why: &str) -> anyhow::Result<()> {
    let Some(_gate) = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))? else {
        return Ok(());
    };
    let Some(report) = read(root)? else { return Ok(()) };
    if report.finished.is_some()
        || (report.process != fastctx_platform::current_identity()? && id != Some(report.id.as_str()))
    {
        return Ok(());
    }
    let mut journal = Journal { root: root.to_owned(), report };
    if journal.report.steps.last().is_none_or(|s| s.outcome != Outcome::Checking) {
        journal.begin("controller")?;
    }
    journal.end(Err(anyhow::anyhow!("{why}")))?;
    journal.finish()
}

/// Appends an acquisition action while this process still owns preparation.
pub fn note(root: &Path, id: &str, description: String) -> anyhow::Result<()> {
    let report = read(root)?.context("repair preparation has no record")?;
    anyhow::ensure!(
        report.id == id && report.finished.is_none() && report.process == fastctx_platform::current_identity()?,
        "repair preparation changed owner"
    );
    Journal { root: root.to_owned(), report }.action(description)
}

/// Reports an interrupted controller only after its identity has ended and its
/// exclusion gate is free. A live or newly starting repair keeps its own record.
pub fn inspect(root: &Path) -> anyhow::Result<Option<Report>> {
    let Some(mut report) = read(root)? else { return Ok(None) };
    if report.finished.is_none() && !fastctx_platform::is_alive(&report.process)? {
        let Some(_gate) = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))? else {
            return Ok(Some(report));
        };
        report = read(root)?.context("repair record disappeared")?;
        if report.finished.is_none() && !fastctx_platform::is_alive(&report.process)? {
            let why = format!("repair process {} ended without a completion receipt", report.process.pid);
            let mut journal = Journal { root: root.to_owned(), report };
            journal.begin("controller")?;
            journal.end(Err(anyhow::anyhow!("{why}")))?;
            journal.finish()?;
            report = journal.report;
        }
    }
    Ok(Some(report))
}
