//! An independent timer and a short check also recover administrative stops.
//! The primary service's enabled state is the desired state: uninstall clears
//! it before stopping anything, so neither a timer nor OnSuccess revives it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use fastctx_platform::atomic::{Permissions, publish};
use serde::{Deserialize, Serialize};

use super::RecoveryStatus;
use super::linux::systemctl;

pub(super) struct Watchdog {
    pub timer: String,
    pub checker: String,
    marker: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Before {
    timer: Option<Vec<u8>>,
    checker: Option<Vec<u8>>,
    enabled: bool,
    active: bool,
}

impl Watchdog {
    pub fn new(main: &str, marker: &str) -> Self {
        let stem = main.strip_suffix(".service").expect("a service unit");
        Self { timer: format!("{stem}-check.timer"), checker: format!("{stem}-check.service"), marker: marker.into() }
    }

    pub fn capture(&self) -> anyhow::Result<Before> {
        let timer = self.read(&self.timer)?;
        let checker = self.read(&self.checker)?;
        let state = self.status()?;
        Ok(Before { timer, checker, enabled: state.enabled, active: state.state == "active" })
    }

    pub fn status(&self) -> anyhow::Result<RecoveryStatus> {
        let text = systemctl(&["show", &self.timer, "--property=ActiveState,UnitFileState"])?;
        let value = |prefix| text.lines().find_map(|line| line.strip_prefix(prefix));
        Ok(RecoveryStatus {
            name: self.timer.clone(),
            enabled: value("UnitFileState=") == Some("enabled"),
            state: value("ActiveState=").context("systemd omitted timer ActiveState")?.into(),
        })
    }

    /// Writes the check for `main` as the running systemd `version` reads
    /// it: 243 brought `ExecCondition=`, 230 the start limit in `[Unit]`;
    /// an older one asks whether `main` is enabled in a shell line.
    fn definitions(&self, main: &str, version: u32) -> anyhow::Result<Vec<(&str, String)>> {
        let systemctl = super::linux_restart::systemctl().context("systemctl is not found")?;
        let systemctl = systemctl.display();
        let checker = if version >= 243 {
            format!(
                "{}[Unit]\nDescription=Check fastctx service\nStartLimitIntervalSec=0\n\n[Service]\nType=oneshot\nExecCondition={systemctl} --system is-enabled --quiet {main}\nExecStart={systemctl} --system start {main}\n",
                self.marker
            )
        } else {
            let limit = if version >= 230 { ("StartLimitIntervalSec=0\n", "") } else { ("", "StartLimitInterval=0\n") };
            format!(
                "{}[Unit]\nDescription=Check fastctx service\n{}\n[Service]\nType=oneshot\n{}ExecStart=/bin/sh -c \"{systemctl} --system is-enabled --quiet {main} && exec {systemctl} --system start {main} || true\"\n",
                self.marker, limit.0, limit.1
            )
        };
        // One minute is the minimum repetition interval of the Windows
        // scheduler. Both platforms use that same secondary-check period.
        let timer = format!(
            "{}[Unit]\nDescription=Check fastctx service every minute\n\n[Timer]\nOnCalendar=*-*-* *:*:00\nAccuracySec=1us\nPersistent=true\nUnit={}\n\n[Install]\nWantedBy=timers.target\n",
            self.marker, self.checker
        );
        Ok(vec![(&self.checker, checker), (&self.timer, timer)])
    }

    pub fn current(&self, main: &str, version: u32) -> anyhow::Result<bool> {
        for (name, text) in self.definitions(main, version)? {
            if self.read(name)?.as_deref() != Some(text.as_bytes()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn write(&self, main: &str, version: u32) -> anyhow::Result<()> {
        for (name, text) in self.definitions(main, version)? {
            if self.read(name)?.as_deref() != Some(text.as_bytes()) {
                publish(&file(name), text.as_bytes(), Permissions::KeepExisting)?;
            }
        }
        Ok(())
    }

    // Enabled and started apart: `--now` came in systemd 220.
    pub fn start(&self) -> anyhow::Result<()> {
        systemctl(&["enable", &self.timer])?;
        systemctl(&["start", &self.timer])?;
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        if self.read(&self.timer)?.is_some() {
            systemctl(&["disable", &self.timer])?;
            systemctl(&["stop", &self.timer])?;
        }
        if self.read(&self.checker)?.is_some() {
            systemctl(&["stop", &self.checker])?;
        }
        Ok(())
    }

    pub fn delete(&self) -> anyhow::Result<()> {
        for name in [&self.timer, &self.checker] {
            self.read(name)?;
            delete(&file(name))?;
        }
        // A persistent timer leaves systemd's record of when it last fired.
        delete(&Path::new("/var/lib/systemd/timers").join(format!("stamp-{}", self.timer)))?;
        Ok(())
    }

    pub fn restore(&self, before: &Before) -> anyhow::Result<()> {
        self.stop()?;
        for (name, bytes) in [(&self.timer, &before.timer), (&self.checker, &before.checker)] {
            match bytes {
                Some(bytes) => publish(&file(name), bytes, Permissions::KeepExisting)?,
                None => delete(&file(name))?,
            }
        }
        systemctl(&["daemon-reload"])?;
        if before.enabled {
            systemctl(&["enable", &self.timer])?;
        }
        if before.active {
            systemctl(&["start", &self.timer])?;
        }
        Ok(())
    }

    fn read(&self, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = file(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(metadata) if !metadata.is_file() => bail!("{} is not an owned regular unit", path.display()),
            Ok(_) => {}
        }
        let bytes = fs::read(&path)?;
        if !bytes.starts_with(self.marker.as_bytes()) {
            bail!("{} is not owned by this service", path.display());
        }
        Ok(Some(bytes))
    }
}

fn file(name: &str) -> PathBuf {
    PathBuf::from("/etc/systemd/system").join(name)
}

fn delete(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
