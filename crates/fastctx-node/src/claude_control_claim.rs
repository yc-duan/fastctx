//! A durable execution claim survives the plugin activation that took it.

use fastctx_hosts::claude::{control, exchange};
use fastctx_types::ProcessIdentity;
use serde::Deserialize;
use std::io::{self, Read};
use std::path::Path;

#[derive(Deserialize)]
struct Request {
    key: String,
    host: control::Host,
    controller: ProcessIdentity,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    release: bool,
}

pub fn run(world: &Path) -> io::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input).map_err(io::Error::other)?;
    crate::claude::pairing::validate_session(&request.host.session)?;
    if request.key.is_empty() || !request.key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(io::Error::other("invalid Claude control key"));
    }
    // A refresh follows only a native precondition proving no compact began.
    if request.refresh {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let directory = control::directory(world, &request.host.session);
    let claims = directory.join("claims");
    fastctx_platform::ensure_private_dir(&claims)?;
    let _lock = fastctx_platform::FileLock::acquire(&claims.join(".lock"))?;
    if control::read::<ProcessIdentity>(&world.join("claude-controller.json"))?.as_ref() != Some(&request.controller)
        || !fastctx_platform::is_alive(&request.controller)?
    {
        return output(false);
    }
    let claim_path = claims.join(format!("{}.json", request.key));
    let prior = control::read::<control::Claim>(&claim_path)?;
    if request.release {
        let Some(mut claim) = prior.filter(|claim| claim.host == request.host) else { return output(false) };
        claim.released = true;
        control::write(&claim_path, &claim)?;
        return output(true);
    }
    if !control::active(&directory, &request.host)? {
        return output(false);
    }
    if request.refresh {
        if prior.as_ref().is_none_or(|claim| claim.host != request.host || claim.released) {
            return output(false);
        }
    } else if prior.as_ref().is_some_and(|claim| !claim.released) {
        return output(false);
    }
    let process = exchange::registration(directory.parent().expect("control directory"))?.and_then(|host| host.process);
    let envelope = control::read_message(&directory.join("inbox").join(format!("{}.json", request.key)))?;
    let evidence = crate::claude::control_outcome::Evidence::capture(&directory, &envelope["control"])?;
    let claim = control::Claim {
        host: request.host,
        released: false,
        process,
        evidence: Some(serde_json::to_value(evidence).map_err(io::Error::other)?),
    };
    // The lock serializes a released claim's takeover with any late release.
    control::write(&claim_path, &claim)?;
    output(true)
}

fn output(claimed: bool) -> io::Result<()> {
    serde_json::to_writer(io::stdout(), &serde_json::json!({"claimed":claimed})).map_err(io::Error::other)
}
