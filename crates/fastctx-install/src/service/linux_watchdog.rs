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

    pub fn write(&self, main: &str) -> anyhow::Result<()> {
        let checker = format!(
            "{}[Unit]\nDescription=Check fastctx service\nStartLimitIntervalSec=0\n\n[Service]\nType=oneshot\nExecCondition=/usr/bin/systemctl --system is-enabled --quiet {}\nExecStart=/usr/bin/systemctl --system start {}\n",
            self.marker, main, main
        );
        // One minute is the minimum repetition interval of the Windows
        // scheduler. Both platforms use that same secondary-check period.
        let timer = format!(
            "{}[Unit]\nDescription=Check fastctx service every minute\n\n[Timer]\nOnCalendar=*-*-* *:*:00\nAccuracySec=1us\nPersistent=true\nUnit={}\n\n[Install]\nWantedBy=timers.target\n",
            self.marker, self.checker
        );
        for (name, text) in [(&self.checker, checker), (&self.timer, timer)] {
            if self.read(name)?.as_deref() != Some(text.as_bytes()) {
                publish(&file(name), text.as_bytes(), Permissions::KeepExisting)?;
            }
        }
        Ok(())
    }

    pub fn start(&self) -> anyhow::Result<()> {
        systemctl(&["enable", "--now", &self.timer])?;
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        if self.read(&self.timer)?.is_some() {
            systemctl(&["disable", "--now", &self.timer])?;
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
