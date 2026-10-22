//! The guardian takes a newer World program by itself (`world-upgrade.md`,
//! 换版不要人确认). Asked by its user, it reads the program its registration
//! names, once, and takes it only when it carries the official signature for
//! a release newer than its own: those bytes become its image, and a hub's
//! program too, in the protected directory. Its user then restarts it, as
//! after an administrator's approval. What its user can write is never run
//! or kept unless that signature checks out over the bytes read.

use std::time::{Duration, Instant};

use anyhow::{Context as _, ensure};
use fastctx_platform::FileLock;
use fastctx_platform::atomic::{Permissions, publish};
use serde::{Deserialize, Serialize};
use windows_service::service::{ServiceAccess, ServiceState, UserEventCode};

use super::{config::Config, scm};
use crate::renewal as images;
use crate::service::{Kind, Service};

/// The user control that asks the guardian to take the newer program.
pub(super) const ADOPT: u32 = 129;
/// The user control that says the next stop is planned: it ends well, and
/// the service manager does not start the guardian again as after a failure.
pub(super) const PLANNED: u32 = 130;

/// How long a user waits for the guardian to take the program.
const TAKING: Duration = Duration::from_secs(60);

/// What the guardian last did when asked, for its user to read.
#[derive(Serialize, Deserialize)]
struct Adopted {
    at: jiff::Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

/// In the guardian: takes the program, and says what it did.
pub(super) fn adopt(config: &Config) -> anyhow::Result<()> {
    let outcome = take(config);
    let adopted = match &outcome {
        Ok(digest) => Adopted { at: jiff::Timestamp::now(), digest: Some(digest.clone()), why: None },
        Err(error) => Adopted { at: jiff::Timestamp::now(), digest: None, why: Some(format!("{error:#}")) },
    };
    publish(&config.root()?.join("adopted.json"), &serde_json::to_vec(&adopted)?, Permissions::KeepExisting)?;
    outcome.map(|_| ())
}

fn take(config: &Config) -> anyhow::Result<String> {
    let program = config.executable();
    let bytes = std::fs::read(&program).with_context(|| format!("reading {}", program.display()))?;
    let signature = std::fs::read_to_string(images::signature_of(&program))
        .context("the World program carries no official signature; an administrator takes it in")?;
    let version = fastctx_update::verify::signed_version(&signature).context("its signature names no release")?;
    fastctx_update::verify::bytes(&bytes, &signature, &version).map_err(anyhow::Error::msg)?;
    let own = env!("CARGO_PKG_VERSION");
    ensure!(
        fastctx_types::update::release(&version) > fastctx_types::update::release(own),
        "{version} is not newer than this guardian's {own}"
    );
    let root = config.root()?;
    let _lock = FileLock::acquire(&root.join("manager.lock"))?;
    super::transaction::require_complete(&root.join("transaction.json"))?;
    let staged = root.join("guardian.adopting");
    publish(&staged, &bytes, Permissions::KeepExisting)?;
    images::publish_image(&staged, &root.join("guardian.exe"))?;
    if config.kind == Kind::Hub {
        images::publish_image(&staged, &root.join("payload.exe"))?;
    }
    let _ = std::fs::remove_file(&staged);
    images::digest(&root.join("guardian.exe")).map_err(Into::into)
}

/// In a hub's guardian, at its check each minute: whether the World program
/// its registration names says by its signature that it is a release newer
/// than this guardian. The guardian then takes it (`take`) and starts again
/// on it by itself, as a hub's user may not ask it to (`ask`), and the hub
/// would go on speaking the old network protocol (`world-upgrade.md`,
/// 中枢跟着换). Only the signature's word is read here; `take` checks it over
/// the program's bytes.
pub(super) fn offered(config: &Config) -> bool {
    let Ok(signature) = std::fs::read_to_string(images::signature_of(&config.executable())) else { return false };
    fastctx_update::verify::signed_version(&signature).is_some_and(|version| {
        fastctx_types::update::release(&version) > fastctx_types::update::release(env!("CARGO_PKG_VERSION"))
    })
}

/// As the guardian's user: asks it to take the program and waits until it
/// has, it says why not, or it does not answer, as a guardian of an earlier
/// release does not. Whether it took the program with `digest`.
pub(crate) fn ask(service: &Service, digest: &str) -> anyhow::Result<bool> {
    if super::renewal::staged(service, digest)? {
        return Ok(true);
    }
    let config = Config::new(service)?;
    let Some(registered) = scm::open(&config.name, ServiceAccess::QUERY_STATUS | ServiceAccess::USER_DEFINED_CONTROL)?
    else {
        return Ok(false);
    };
    if registered.query_status()?.current_state != ServiceState::Running {
        return Ok(false);
    }
    let asked = jiff::Timestamp::now();
    registered.notify(UserEventCode::from_raw(ADOPT).context("the adoption control")?)?;
    let began = Instant::now();
    while began.elapsed() < TAKING {
        if super::renewal::staged(service, digest)? {
            return Ok(true);
        }
        let said: Option<Adopted> = images::read_json(&config.root()?.join("adopted.json"))?;
        if let Some(Adopted { at, why: Some(why), .. }) = said
            && at >= asked
        {
            anyhow::bail!("the boot service did not take the new program: {why}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(false)
}

/// As the guardian's user: says the stop that follows is planned. A guardian
/// of an earlier release takes it as a check.
pub(super) fn planned(registered: &windows_service::service::Service) {
    if let Ok(code) = UserEventCode::from_raw(PLANNED) {
        let _ = registered.notify(code);
    }
}
