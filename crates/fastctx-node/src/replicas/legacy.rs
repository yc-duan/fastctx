//! Copies kept by releases before format 3 held their texts in their
//! metadata. Reading one keeps its texts as bodies, keeps the original under
//! the World's archive for its retention, and writes the copy anew.

use super::{
    bodies::{self, Text},
    state::{self, Copy, Observed, Pending},
};
use fastctx_types::{access::HubAccount, history::Author, layout::WorldDir, replicas::Head, write::WriteOp};
use serde::Deserialize;
use std::{collections::BTreeMap, io, path::Path};

#[derive(Deserialize)]
struct LegacyPending {
    id: String,
    base: Option<u64>,
    before: String,
    text: String,
    operation: WriteOp,
    author: Author,
    offline: bool,
}

#[derive(Deserialize)]
struct LegacyObserved {
    version: Option<u64>,
    hash: Option<String>,
}

#[derive(Deserialize)]
struct LegacyCopy {
    account: HubAccount,
    head: Head,
    path: std::path::PathBuf,
    text: String,
    #[serde(default)]
    observed: BTreeMap<String, LegacyObserved>,
    #[serde(default)]
    pending: Vec<LegacyPending>,
}

/// Removes the original records of a withdrawn copy from every release's archive.
pub(crate) fn delete(world: &WorldDir, id: &str) -> io::Result<()> {
    let versions = match fastctx_platform::disk::read_dir(world.path().join("archive")) {
        Ok(versions) => versions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for version in versions {
        let version = version?;
        if version.file_type()?.is_dir() {
            super::files::delete(&version.path().join("replicas").join("copies").join(format!("{id}.json")))?;
        }
    }
    Ok(())
}

/// The copy kept at `path`, in this release's form.
pub(crate) fn read(world: &WorldDir, path: &Path) -> io::Result<Copy> {
    let bytes = fastctx_platform::disk::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value.get("body").is_some() {
        return Ok(serde_json::from_value(value)?);
    }
    let legacy: LegacyCopy = serde_json::from_value(value)?;
    let mut copy = Copy {
        epoch: 0,
        account: legacy.account,
        head: legacy.head,
        path: legacy.path,
        body: Text::default(),
        projected: None,
        observed: BTreeMap::new(),
        pending: Vec::new(),
        left: Vec::new(),
    };
    let id = copy.id();
    copy.body = bodies::put(world, &id, &legacy.text)?;
    for pending in legacy.pending {
        copy.pending.push(Pending {
            id: pending.id,
            base: pending.base,
            before: bodies::put(world, &id, &pending.before)?,
            text: bodies::put(world, &id, &pending.text)?,
            edit: (!matches!(pending.operation, WriteOp::Write(_))).then_some(pending.operation),
            author: pending.author,
            offline: pending.offline,
        });
    }

    copy.observed = legacy
        .observed
        .into_iter()
        .map(|(owner, seen)| (owner, Observed { version: seen.version, hash: seen.hash }))
        .collect();
    let archived = crate::record_migration::archive::root(world.path())
        .join("replicas")
        .join("copies")
        .join(path.file_name().ok_or_else(|| io::Error::other("a replica record has no filename"))?);
    fastctx_platform::ensure_private_dir(archived.parent().expect("archive directory"))?;
    match fastctx_platform::publish_new(&archived, &bytes, fastctx_platform::Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    state::save(world, &copy)?;
    Ok(copy)
}
