//! What a Codex login's app-server says of it whenever one starts for the
//! World's own work (`fastctx_hosts::codex::identify::SaidReport`): who is
//! logged in, its models and its program's version, kept as asking the
//! login keeps them. The record so follows the login and its program as
//! they change, an upgrade of the desktop app first of all, with nothing
//! started or asked for it. An answer that failed keeps what was known.

use std::sync::Arc;

use fastctx_hosts::codex::identify::SaidReport;
use fastctx_types::agents::{Fact, ProgramSource};
use fastctx_types::logins::{Identified, Launcher};
use tokio::sync::mpsc;

use super::Context;
use super::kept::{self, FileStamp, KeptLogin};
use crate::Node;

/// Keeps every report as it comes.
pub(crate) fn listen(node: Arc<Node>, mut reports: mpsc::UnboundedReceiver<SaidReport>) {
    tokio::spawn(async move {
        while let Some(report) = reports.recv().await {
            let keeping = node.clone();
            match tokio::task::spawn_blocking(move || keep(&keeping, report)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => node.log.line(format_args!("keeping what a Codex login said: {error}")),
                Err(error) => node.log.line(format_args!("keeping what a Codex login said: {error}")),
            }
        }
    });
}

/// Records one report, for a login the World knows here.
fn keep(node: &Node, report: SaidReport) -> Result<(), String> {
    let SaidReport { home, launch, at, said } = report;
    let context = Context::of(node).map_err(|failure| failure.message)?;
    let record = kept::read(node).map_err(|error| format!("reading the agents' logins: {error}"))?;
    let found = super::dirs(node, &context, &record, "codex")
        .map_err(|failure| failure.message)?
        .into_iter()
        .find(|(dir, _)| crate::homes::same_dir(dir, &home))
        .map(|(_, found)| found);
    let Some(found) = found else { return Ok(()) };
    let launcher = Launcher {
        path: launch.binary,
        version: said.version,
        desktop_app: launch.source == ProgramSource::DesktopApp,
    };
    let asked_file = FileStamp::of(&launcher.path);
    let signed = super::sign_in::now(&context, "codex", &home);
    let models = match said.models {
        Fact::Known { value } => Some(value),
        _ => None,
    };
    kept::change(node, |record| {
        let logins = &mut record.codex.logins;
        match logins.iter_mut().find(|login| crate::homes::same_dir(&login.dir, &home)) {
            Some(login) => {
                if at >= login.identified.at && matches!(said.who, Fact::Known { .. }) {
                    login.identified = Identified { at, who: said.who };
                    login.signed = Some(signed);
                }
                if let Some(models) = models {
                    login.models = models;
                }
                login.launcher = Some(launcher);
                login.asked_file = asked_file;
            }
            None => logins.push(KeptLogin {
                dir: home,
                found,
                identified: Identified { at, who: said.who },
                launcher: Some(launcher),
                asked_file,
                models: models.unwrap_or_default(),
                signed: Some(signed),
            }),
        }
    })
    .map_err(|error| format!("keeping the agents' logins: {error}"))
}
