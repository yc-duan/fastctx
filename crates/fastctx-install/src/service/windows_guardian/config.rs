//! Only administrators write this launch description. Paths keep their native
//! UTF-16 code units; the SCM command line contains just a validated service id.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::PathBuf;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use super::super::{Kind, Service};
use super::{account, security};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Config {
    pub name: String,
    pub kind: Kind,
    pub executable: Vec<u16>,
    pub directory: Vec<u16>,
    pub target: Option<account::Target>,
}

impl Config {
    pub fn new(service: &Service) -> anyhow::Result<Self> {
        // A node SYSTEM installs for itself is SYSTEM's, as the hub always is.
        let target = match service.kind {
            Kind::Node if !for_system(service)? => Some(account::target(service.user_sid.as_deref())?),
            _ => None,
        };
        let account = target.as_ref().map(|target| target.sid.as_str()).unwrap_or("S-1-5-18");
        Ok(Self {
            name: service.id(account.as_bytes()),
            kind: service.kind,
            executable: service.executable.as_os_str().encode_wide().collect(),
            directory: service.directory.as_os_str().encode_wide().collect(),
            target,
        })
    }

    /// Whether the node runs as SYSTEM, in the home Fastctx gives SYSTEM.
    pub fn systems(&self) -> bool {
        self.kind == Kind::Node && self.target.is_none()
    }

    pub fn root(&self) -> anyhow::Result<PathBuf> {
        security::directory(&self.name)
    }
    pub fn executable(&self) -> PathBuf {
        OsString::from_wide(&self.executable).into()
    }
    pub fn directory(&self) -> PathBuf {
        OsString::from_wide(&self.directory).into()
    }

    pub fn read(name: &str) -> anyhow::Result<Self> {
        let bytes = std::fs::read(security::directory(name)?.join("config.json"))?;
        let config: Self = serde_json::from_slice(&bytes)?;
        if config.name != name || (config.kind == Kind::Hub && config.target.is_some()) {
            bail!("guardian configuration does not identify this service");
        }
        if !config.executable().is_absolute() || !config.directory().is_absolute() {
            bail!("guardian paths must be absolute");
        }
        let account = config.target.as_ref().map(|target| target.sid.as_str()).unwrap_or("S-1-5-18");
        if Service::new(config.kind, config.executable(), config.directory()).id(account.as_bytes()) != name {
            bail!("guardian account and directory do not match its registration");
        }
        Ok(config)
    }
}

fn for_system(service: &Service) -> anyhow::Result<bool> {
    let sid = match &service.user_sid {
        Some(sid) => sid.clone(),
        None => fastctx_platform::current_user_sid()?,
    };
    Ok(sid == SYSTEM)
}

const SYSTEM: &str = "S-1-5-18";
