//! What a conversation moving to another machine carries of its streams
//! (design §28.10): for each stream this machine orders in the
//! conversation's scope, a release of it to the machine named — bound to
//! that machine's device key as this machine last saw the hub's directory —
//! under this machine's device key, the frames of it the hub has not taken
//! yet, and the objects those frames refer to. Written beside the
//! conversation as it is exported, the source node stopped; read on the
//! machine it goes to, where it lets that machine take each stream over once
//! it sent the frames and their objects on. Each export writes it anew or
//! takes the last one away: a package is never left from an earlier export
//! (AP07).

use fastctx_content::v2::Library;
use fastctx_net::sign::{Domain, SecretKey};
use fastctx_types::assets::{Frame, Release};
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// The file the package is kept in, in the conversation's directory.
pub(crate) const FILE: &str = "streams-released.json";

/// The folder beside it that holds the objects its frames refer to, each
/// named by its hash.
pub(crate) const OBJECTS: &str = "streams-released";

/// How much of an object is read or written at once.
const PIECE: u32 = 1 << 20;

/// What the conversation carries of its streams.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Package {
    pub hub: String,
    pub scope: String,
    pub to: String,
    /// The device key of the machine it goes to.
    pub to_key: fastctx_types::assets::Bytes,
    /// The device key of the machine that let it go.
    #[serde(default)]
    pub from_key: fastctx_types::assets::Bytes,
    pub streams: Vec<Released>,
}

/// One stream let go: the release, the frames to send on before it, and
/// the objects they refer to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Released {
    pub stream: String,
    pub release: Release,
    pub tail: Vec<Frame>,
    /// The hashes of the objects the frames refer to.
    #[serde(default)]
    pub objects: Vec<fastctx_types::assets::Bytes>,
}

/// Takes away what an earlier export left beside the conversation: its
/// package, and an adopting not done, which was this machine's (AQ02).
pub(crate) fn clear(directory: &Path) -> io::Result<()> {
    clear_package(directory)?;
    match fastctx_platform::disk::remove_file(directory.join(ADOPTED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Takes away a package used up here, and its objects; an adopting not done
/// stays (NN12).
pub(crate) fn clear_package(directory: &Path) -> io::Result<()> {
    match fastctx_platform::disk::remove_file(directory.join(FILE)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    match fastctx_platform::disk::remove_dir_all(directory.join(OBJECTS)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Writes what `conversation` carries of its streams, going to the machine
/// `to`, beside it in `world`; none when this machine orders none of them.
/// The node is stopped.
pub(crate) fn export(world: &WorldDir, directory: &Path, conversation: &str, to: &str) -> io::Result<Option<PathBuf>> {
    let key = crate::world::key(world)?;
    let library = super::Assets::open_stopped(world)?;
    let Some((hub, scope)) = scope_of(&library, conversation)? else {
        clear(directory)?;
        return Ok(None);
    };
    // Made whole beside it before what an earlier export left goes: one
    // that fails at any step leaves that one as it was (K06, NN15, L03).
    let to_key = member_key(world, &hub, to)?;
    let package = package(&library, &key, &hub, &scope, to, to_key)?;
    if package.streams.is_empty() {
        clear(directory)?;
        return Ok(None);
    }
    let (folder, path) = (directory.join(OBJECTS), directory.join(FILE));
    let (new_folder, new_path) = (directory.join(format!("{OBJECTS}.new")), directory.join(format!("{FILE}.new")));
    let made = (|| -> io::Result<()> {
        match fastctx_platform::disk::remove_dir_all(&new_folder) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        fastctx_platform::disk::create_dir_all(&new_folder)?;
        for object in package.streams.iter().flat_map(|released| &released.objects) {
            copy_out(&library, &object.0, &new_folder)?;
        }
        let bytes = serde_json::to_vec_pretty(&package).map_err(io::Error::other)?;
        fastctx_platform::publish(&new_path, &bytes, fastctx_platform::Permissions::Private)
    })();
    if let Err(error) = made {
        let _ = fastctx_platform::disk::remove_dir_all(&new_folder);
        let _ = fastctx_platform::disk::remove_file(&new_path);
        return Err(error);
    }
    clear(directory)?;
    fastctx_platform::disk::rename(&new_folder, &folder)?;
    fastctx_platform::disk::rename(&new_path, &path)?;
    Ok(Some(path))
}

/// The device key of the machine `to` — by its name or one it had — as
/// this machine last saw `hub`'s directory.
fn member_key(world: &WorldDir, hub: &str, to: &str) -> io::Result<[u8; 32]> {
    let bytes = fastctx_platform::disk::read(world.net().join(hub).join("view.json"))?;
    let view: fastctx_net::directory::View = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    view.members
        .iter()
        .find(|member| member.name.as_str() == to || member.was.iter().any(|was| was.as_str() == to))
        .map(|member| *member.key.as_bytes())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no machine {to} is on the hub's directory as this machine last saw it")))
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
fn package(library: &Library, key: &SecretKey, hub: &str, scope: &str, to: &str, to_key: [u8; 32]) -> io::Result<Package> {
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
            to_key,
            next,
            head,
        };
        let signature = key.sign(Domain::AssetRelease, &release.statement()).to_bytes();
        let mut objects: Vec<fastctx_types::assets::Bytes> = Vec::new();
        let tail = library
            .export(scope, &stream, uploaded, usize::MAX)
            .map_err(error)?
            .into_iter()
            .map(|frame| {
                for object in &frame.objects {
                    let object = fastctx_types::assets::Bytes(object.to_vec());
                    if !objects.contains(&object) {
                        objects.push(object);
                    }
                }
                Frame::whole(frame.header, frame.signature, frame.body)
            })
            .collect();
        streams.push(Released {
            stream,
            release: Release {
                epoch,
                to: to.to_string(),
                to_key: to_key.to_vec().into(),
                next,
                head: head.to_vec().into(),
                signature: signature.to_vec().into(),
            },
            tail,
            objects,
        });
    }
    Ok(Package {
        hub: hub.to_string(),
        scope: scope.to_string(),
        to: to.to_string(),
        to_key: to_key.to_vec().into(),
        from_key: device.to_vec().into(),
        streams,
    })
}

/// Writes the object `hash` of the library into `folder`, named by it.
fn copy_out(library: &Library, hash: &[u8], folder: &Path) -> io::Result<()> {
    let hash: [u8; 32] = hash.try_into().map_err(|_| io::Error::other("a malformed object hash"))?;
    fastctx_platform::disk::create_dir_all(folder)?;
    let mut file = fastctx_platform::disk::create(folder.join(hex(&hash)))?;
    let mut at = 0u64;
    loop {
        let piece = library.read(&hash, at, PIECE).map_err(|error| io::Error::other(error.to_string()))?;
        file.write_all(&piece)?;
        at += piece.len() as u64;
        if piece.len() < PIECE as usize {
            break;
        }
    }
    file.sync_all()
}

/// Where the object `hash` of the package is beside the conversation, if it
/// came.
pub(crate) fn object_path(directory: &Path, hash: &[u8]) -> PathBuf {
    directory.join(OBJECTS).join(hex(hash))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What `directory` carries of `scope`'s `stream` for this machine, whose
/// device key is `me`, on `hub`, if it carries it.
pub(crate) fn carried(directory: &Path, hub: &str, scope: &str, stream: &str, me: &[u8; 32]) -> Option<Released> {
    let bytes = fastctx_platform::disk::read(directory.join(FILE)).ok()?;
    let package: Package = serde_json::from_slice(&bytes).ok()?;
    if package.hub != hub || package.scope != scope || package.to_key.0.as_slice() != me.as_slice() {
        return None;
    }
    package.streams.into_iter().find(|released| released.stream == stream)
}

/// The file that says the account that opened the conversation has it
/// written on here from now on (design §28.10): kept beside it until each
/// of its streams is taken over, so a node that stops first goes on with it
/// when it starts (LL09).
pub(crate) const ADOPTED: &str = "streams-adopted.json";

/// What adopting a conversation keeps beside it: on which hub, which scope,
/// and the device key of the machine whose account adopted it there —
/// carried to another machine, it is that one's no more (AQ02).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Adopting {
    hub: String,
    scope: String,
    #[serde(default)]
    by: String,
}

/// Keeps that `scope` of `hub` is adopted on this machine, `me`.
pub(crate) fn adopt(directory: &Path, hub: &str, scope: &str, me: &[u8; 32]) -> io::Result<()> {
    let adopting = Adopting { hub: hub.to_string(), scope: scope.to_string(), by: hex(me) };
    let bytes = serde_json::to_vec(&adopting).map_err(io::Error::other)?;
    fastctx_platform::publish(&directory.join(ADOPTED), &bytes, fastctx_platform::Permissions::Private)
}

/// Whether `scope` of `hub` is being adopted on this machine, `me`.
pub(crate) fn adopting(directory: &Path, hub: &str, scope: &str, me: &[u8; 32]) -> bool {
    fastctx_platform::disk::read(directory.join(ADOPTED))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Adopting>(&bytes).ok())
        .is_some_and(|adopting| adopting.hub == hub && adopting.scope == scope && adopting.by == hex(me))
}

/// Adopting is over: each stream taken over, or the conversation gone.
pub(crate) fn adopted(directory: &Path) {
    let _ = fastctx_platform::disk::remove_file(directory.join(ADOPTED));
}

/// Whether this machine, `me`, let `scope` go to another machine with the
/// conversation in `directory`: exported with its release, it writes none
/// of the scope's streams any more, which the machine it goes to takes over
/// from where they ended (J08). Exported again without a release, or
/// adopted here by the account's word, it does.
pub(crate) fn released_away(directory: &Path, scope: &str, me: &[u8; 32]) -> bool {
    let adopted = fastctx_platform::disk::read(directory.join(ADOPTED))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Adopting>(&bytes).ok())
        .is_some_and(|adopting| adopting.scope == scope && adopting.by == hex(me));
    !adopted
        && fastctx_platform::disk::read(directory.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Package>(&bytes).ok())
            .is_some_and(|package| {
                package.scope == scope && package.from_key.0.as_slice() == me.as_slice() && package.to_key.0.as_slice() != me.as_slice()
            })
}

/// The streams the package beside the conversation releases to this
/// machine, `me`: used up once each is ordered here, when it goes.
pub(crate) fn released_here(directory: &Path, scope: &str, me: &[u8; 32]) -> Vec<String> {
    fastctx_platform::disk::read(directory.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Package>(&bytes).ok())
        .filter(|package| package.scope == scope && package.to_key.0.as_slice() == me.as_slice())
        .map(|package| package.streams.into_iter().map(|released| released.stream).collect())
        .unwrap_or_default()
}
