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
}

pub fn run(world: &Path) -> io::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input).map_err(io::Error::other)?;
    crate::claude::pairing::validate_session(&request.host.session)?;
    if request.key.is_empty() || !request.key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(io::Error::other("invalid Claude control key"));
    }
    let directory = control::directory(world, &request.host.session);
    if control::read::<ProcessIdentity>(&world.join("claude-controller.json"))?.as_ref() != Some(&request.controller)
        || !fastctx_platform::is_alive(&request.controller)?
        || !control::active(&directory, &request.host)?
    {
        return serde_json::to_writer(io::stdout(), &serde_json::json!({"claimed":false})).map_err(io::Error::other);
    }
    let claims = directory.join("claims");
    fastctx_platform::ensure_private_dir(&claims)?;
    let process = exchange::registration(directory.parent().expect("control directory"))?.and_then(|host| host.process);
    let envelope = control::read_message(&directory.join("inbox").join(format!("{}.json", request.key)))?;
    let evidence = crate::claude::control_outcome::Evidence::capture(&directory, &envelope["control"])?;
    let claim = control::Claim {
        host: request.host,
        process,
        evidence: Some(serde_json::to_value(evidence).map_err(io::Error::other)?),
    };
    let claimed = match fastctx_platform::publish_new(
        &claims.join(format!("{}.json", request.key)),
        &serde_json::to_vec(&claim).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    ) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    serde_json::to_writer(io::stdout(), &serde_json::json!({"claimed":claimed})).map_err(io::Error::other)
}
