//! What a conversation moving to another machine carries of its streams
//! (design §28.10): for each stream this machine orders in the
//! conversation's scope, a release of it to the machine named, under this
//! machine's device key, and the frames of it the hub has not taken yet.
//! Written beside the conversation as it is exported, the source node
//! stopped; read on the machine it goes to, where it lets that machine take
//! each stream over once it sent the frames on.

use fastctx_content::v2::Library;
use fastctx_net::sign::{Domain, SecretKey};
use fastctx_types::assets::{Frame, Release};
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The file the package is kept in, in the conversation's directory.
pub(crate) const FILE: &str = "streams-released.json";

/// What the conversation carries of its streams.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Package {
    pub hub: String,
    pub scope: String,
    pub to: String,
    pub streams: Vec<Released>,
}

/// One stream let go: the release, and the frames to send on before it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Released {
    pub stream: String,
    pub release: Release,
    pub tail: Vec<Frame>,
}

/// Writes what `conversation` carries of its streams, going to the machine
/// `to`, beside it in `world`; none when this machine orders none of them.
/// The node is stopped.
pub(crate) fn export(world: &WorldDir, directory: &Path, conversation: &str, to: &str) -> io::Result<Option<PathBuf>> {
    let key = crate::world::key(world)?;
    let library = super::Assets::open_stopped(world)?;
    let Some((hub, scope)) = scope_of(&library, conversation)? else { return Ok(None) };
    let package = package(&library, &key, &hub, &scope, to)?;
    if package.streams.is_empty() {
        return Ok(None);
    }
    let bytes = serde_json::to_vec_pretty(&package).map_err(io::Error::other)?;
    let path = directory.join(FILE);
    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)?;
    Ok(Some(path))
}

/// The hub and scope `conversation` keeps its streams under here.
fn scope_of(library: &Library, conversation: &str) -> io::Result<Option<(String, String)>> {
    let error = |error: fastctx_content::Error| io::Error::other(error.to_string());
    let ending = format!("/{conversation}");
    for hub in library.joined().map_err(error)? {
        for (scope, _, _) in library.live_scopes_of(&hub).map_err(error)? {
            if scope.ends_with(&ending) {
                return Ok(Some((hub, scope)));
            }
        }
    }
    Ok(None)
}

/// The releases of the streams of `scope` this machine orders, to `to`.
fn package(library: &Library, key: &SecretKey, hub: &str, scope: &str, to: &str) -> io::Result<Package> {
    let error = |error: fastctx_content::Error| io::Error::other(error.to_string());
    let device = *key.public().as_bytes();
    let mut streams = Vec::new();
    for fastctx_content::v2::Releasable { stream, epoch, next, head, uploaded } in library.releasable(scope, &device).map_err(error)? {
        let release = fastctx_content::v2::Release {
            hub: hub.to_string(),
            scope: scope.to_string(),
            stream: stream.clone(),
            epoch,
            to: to.to_string(),
            next,
            head,
        };
        let signature = key.sign(Domain::AssetRelease, &release.statement()).to_bytes();
        let tail = library
            .export(scope, &stream, uploaded, usize::MAX)
            .map_err(error)?
            .into_iter()
            .map(|frame| Frame::whole(frame.header, frame.signature, frame.body))
            .collect();
        streams.push(Released {
            stream,
            release: Release { epoch, to: to.to_string(), next, head: head.to_vec().into(), signature: signature.to_vec().into() },
            tail,
        });
    }
    Ok(Package { hub: hub.to_string(), scope: scope.to_string(), to: to.to_string(), streams })
}

/// What `directory` carries of `scope`'s `stream` for this machine, `me`,
/// on `hub`, if it carries it.
pub(crate) fn carried(directory: &Path, hub: &str, scope: &str, stream: &str, me: &str) -> Option<Released> {
    let bytes = fastctx_platform::disk::read(directory.join(FILE)).ok()?;
    let package: Package = serde_json::from_slice(&bytes).ok()?;
    if package.hub != hub || package.scope != scope || package.to != me {
        return None;
    }
    package.streams.into_iter().find(|released| released.stream == stream)
}
