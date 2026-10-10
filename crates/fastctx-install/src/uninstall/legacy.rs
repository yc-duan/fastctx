//! The unpublished node registrations used a user's home, not FASTCTX_HOME.

use super::Installation;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Legacy {
    pub name: String,
    pub path: Option<PathBuf>,
    pub definition: String,
}

pub fn legacy(installation: &Installation) -> anyhow::Result<Vec<Legacy>> {
    if installation.root != installation.home.join(".fastctx") {
        return Ok(Vec::new());
    }
    #[cfg(windows)]
    return windows::find(installation);
    #[cfg(target_os = "linux")]
    return linux::find(installation);
    #[cfg(not(any(windows, target_os = "linux")))]
    Ok(Vec::new())
}

impl Legacy {
    pub fn uninstall(&self, installation: &Installation) -> anyhow::Result<()> {
        anyhow::ensure!(
            legacy(installation)?.iter().any(|found| found.name == self.name && found.definition == self.definition),
            "the legacy registration changed after preview; it was left untouched"
        );
        #[cfg(windows)]
        {
            super::super::service::checked(
                std::process::Command::new("schtasks.exe").args(["/Delete", "/TN", &self.name, "/F"]),
            )?;
        }
        #[cfg(target_os = "linux")]
        {
            super::super::service::checked(
                std::process::Command::new("systemctl").args(["--user", "disable", "--now", &self.name]),
            )?;
            if let Some(path) = &self.path {
                super::delete_file(path)?;
            }
            super::super::service::checked(std::process::Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        }
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::Context as _;
    use std::process::Command;

    pub(super) fn find(installation: &Installation) -> anyhow::Result<Vec<Legacy>> {
        let script = "$ErrorActionPreference='Stop'; $s=New-Object -ComObject Schedule.Service; $s.Connect(); @($s.GetFolder('\\').GetTasks(1) | Where-Object { $_.Name -eq 'FastCtx Node' -or $_.Name -match '^FastCtx Node [0-9a-f]{12}$' } | ForEach-Object { @{name=$_.Path; xml=$_.Xml} }) | ConvertTo-Json -Depth 4 -Compress";
        let text = super::super::super::service::checked(Command::new("powershell.exe").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]))?;
        let values: Vec<serde_json::Value> = serde_json::from_str(&text)?;
        let mut found = Vec::new();
        for value in values {
            let name = value["name"].as_str().context("legacy task has no name")?;
            let xml = value["xml"].as_str().context("legacy task has no XML")?;
            let document = roxmltree::Document::parse(xml)?;
            let text = |parent: &str, child: &str| {
                document
                    .descendants()
                    .find(|node| node.has_tag_name(parent))
                    .and_then(|node| node.children().find(|node| node.has_tag_name(child)))
                    .and_then(|node| node.text())
            };
            if text("Principal", "UserId") != Some(installation.user_sid.as_str()) {
                continue;
            }
            let Some(command) = text("Exec", "Command") else { continue };
            let Some(arguments) = text("Exec", "Arguments") else { continue };
            if name == "\\FastCtx Node" {
                if arguments != "node run" || installation.root.join("bin/fastctx.exe") != std::path::Path::new(command) {
                    continue;
                }
            } else {
                let home = installation.home.to_str().context("legacy task home is not Unicode")?;
                let quoted = format!("--home \"{}\"", home.replace('/', "\\").trim_end_matches('\\'));
                if !arguments.contains(" node run --service-host ") || !arguments.replace('/', "\\").ends_with(&quoted)
                {
                    continue;
                }
                if !PathBuf::from(command).file_name().is_some_and(|name| name.eq_ignore_ascii_case("conhost.exe")) {
                    continue;
                }
            }
            found.push(Legacy { name: name.into(), path: None, definition: xml.into() });
        }
        Ok(found)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    pub(super) fn find(installation: &Installation) -> anyhow::Result<Vec<Legacy>> {
        let mut found = Vec::new();
        let directory = installation.home.join(".config/systemd/user");
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
            Err(error) => return Err(error.into()),
        };
        let quote = |path: &std::path::Path| {
            format!(
                "\"{}\"",
                path.display()
                    .to_string()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('%', "%%")
                    .replace('$', "$$")
            )
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name != "fastctx-node.service" && !(name.starts_with("fastctx-node-") && name.ends_with(".service")) {
                continue;
            }
            let text = std::fs::read_to_string(entry.path())?;
            let installed = installation.root.join("bin/fastctx");
            let scoped = format!("ExecStart={} node run --home {}", quote(&installed), quote(&installation.home));
            let plain = format!("ExecStart={} node run", installed.display());
            if !text.contains("Description=FastCtx World node")
                || !text.lines().any(|line| line == scoped || line == plain)
            {
                continue;
            }
            found.push(Legacy { name: name.into(), path: Some(entry.path()), definition: text });
        }
        Ok(found)
    }
}
