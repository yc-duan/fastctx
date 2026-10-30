//! Immutable evidence folders use the same account, location and Hands gate as
//! mutable working copies. Their bodies travel from memory to the working node.
use crate::{Node, admission::Asker};
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubAccount, Purpose},
    ipc::Failure,
    placement::{Location, Resource},
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
    skill_files::File,
};
use std::sync::Arc;

pub(crate) struct Lease {
    pub location: Location,
    scope: HubAccount,
    owner: Address,
    version: u64,
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
pub(crate) async fn lease(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    acting: &Acting,
) -> Result<Lease, Failure> {
    let location = super::location::prepare(node, id, origin, acting)
        .await?
        .ok_or_else(|| Failure::not_run("there is no online working computer for the evidence files"))?;
    let scope =
        crate::plan::file_account(node, id, acting)?.ok_or_else(|| failure("the evidence needs one owning account"))?;
    let link = node.link_of(&scope.hub).ok_or_else(|| failure("the evidence hub is unavailable"))?;
    let owner = Address { machine: node.machine(), resident: id.clone() };
    let (version, place) = super::location::read_place(&link, &scope, &owner).await?;
    if place.is_none_or(|place| place.machine.as_ref() != Some(&location.machine)) {
        return Err(Failure::not_run("the working location changed before evidence preparation"));
    }
    Ok(Lease {
        location,
        scope,
        owner,
        version: version.ok_or_else(|| failure("the working location has no version"))?,
    })
}
impl Lease {
    pub(crate) fn file(&self, key: &str, name: &str) -> Result<String, Failure> {
        validate_key(key)?;
        fastctx_types::skill_files::validate_path(name).map_err(failure)?;
        Ok(format!("{}/artifacts/{key}/{name}", self.location.path.trim_end_matches('/')))
    }
    pub(crate) async fn publish(&self, node: &Arc<Node>, key: String, files: Vec<File>) -> Result<Location, Failure> {
        let resource = Resource::Files { key, files };
        if node.is_me(&self.location.machine) {
            let asker = Asker {
                machine: node.machine(),
                hub: self.scope.hub.clone(),
                account: self.scope.account.clone(),
                hands: true,
            };
            return super::prepare_copy(node, &asker, self.owner.clone(), self.version, resource).await;
        }
        let link = node.link_of(&self.scope.hub).ok_or_else(|| failure("the evidence hub is unavailable"))?;
        match link
            .call(
                &self.location.machine,
                &self.scope.account,
                Purpose::Hands,
                RemoteCall::WorkingCopy { owner: self.owner.clone(), placement: self.version, resource },
            )
            .await
            .map_err(failure)??
        {
            RemoteReply::WorkingCopy { location } => Ok(location),
            _ => Err(failure("the working computer did not acknowledge the evidence folder")),
        }
    }
}
/// The existing observation sandbox exposes only its script and current
/// directory. A goal therefore starts in this verified evidence-only subtree.
pub(crate) fn read_directory(
    node: &Node,
    scope: &HubAccount,
    owner: &Address,
) -> Result<Option<std::path::PathBuf>, Failure> {
    let room = node.world.owner(&node.names(), owner);
    let path = room.path().join("artifacts");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(error)),
    };
    super::ownership::verify(node, room.path(), scope, owner)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Failure::refused("the evidence workspace is not a plain directory"));
    }
    Ok(Some(path))
}

fn validate_key(key: &str) -> Result<(), Failure> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(failure("invalid artifact folder key"));
    }
    Ok(())
}
pub(crate) async fn put(
    node: &Arc<Node>,
    directory: std::path::PathBuf,
    key: String,
    files: Vec<File>,
) -> Result<Location, Failure> {
    validate_key(&key)?;
    let size = fastctx_types::skill_files::validate_files(&files).map_err(failure)?;
    let maximum = (fastctx_platform::memory::physical().map_err(failure)? / 128)
        .min(fastctx_platform::space::available(&directory).map_err(failure)? / 128);
    if size > maximum {
        return Err(failure(format!("the evidence folder exceeds this computer's {maximum}-byte allowance")));
    }
    let target = directory.join("artifacts").join(&key);
    let gate = node.places.using(&format!("artifact-{}-{key}", directory.to_string_lossy())).await;
    let location = Location { machine: node.machine(), path: crate::world::native(&target) };
    tokio::task::spawn_blocking(move || {
        if target.exists() {
            if super::files::complete(&target, &files).map_err(failure)? {
                return Ok(());
            }
            return Err(failure("an immutable evidence folder already holds different content"));
        }
        let stage = target.with_file_name(format!(".{key}.arriving"));
        super::files::erase(&stage).map_err(failure)?;
        super::files::unpack(&stage, &files).map_err(failure)?;
        std::fs::rename(stage, target).map_err(failure)
    })
    .await
    .map_err(failure)??;
    drop(gate);
    Ok(location)
}
