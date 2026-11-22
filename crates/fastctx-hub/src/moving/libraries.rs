//! The owner libraries after the switch: sent by the old place, one at a
//! time, to the hub that now serves.
//!
//! A move stops the hub for its core alone (`files::Part`). The owner
//! libraries, the accounts' and the bodies', stay where the hub was until
//! it serves no more: from then nothing writes them, so each is sent once
//! and is what it was. The old place asks the new hub which are still to
//! come and sends the databases of each, the smallest first, each written
//! out as one that holds no large object (`snapshot`); the new hub checks
//! what it holds of it against the digest of what was sent, and opens it.
//! Until then the new hub opens no library of that owner, nor makes one
//! (`assets::owners`): a request for it fails and is asked again, as one a
//! busy hub did not answer is. What the hub's own records hold — who is a
//! member, which logins assist whom and with what allowance — came with its
//! core; conversations are kept and read on their machines; what waits for
//! a library is what the hub keeps a copy of for the other devices of an
//! account.
//!
//! Once every library that could be sent is open there, the large objects
//! follow, library by library: those a frame, a record or a hold still
//! refers to, each put into its library as one a device sends is. Until one
//! came the library says it does not hold it, as is so, and a device that
//! refers to it anew is asked for its bytes. The old place's data is the
//! only copy of them until the last came (`follow::delete_old`).
//!
//! A library that cannot be read here, or that the new hub does not take,
//! does not keep the others waiting: it is passed by for the round and goes
//! after those that never failed in the next.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fastctx_net::dial::HubSocket;
use fastctx_net::ids::CertPin;
use fastctx_net::moving::HandoverRole;
use fastctx_net::sign::{Digest, Domain, Signature};
use fastctx_net::wire::Refusal;

use super::files::{self, Part};
use super::frames::{self, Got, ToNew, ToOld};
use super::{Outgoing, standby};
use crate::log::Log;
use crate::{Hub, Socket, refuse};

/// How many large objects are asked after at once.
const FOLLOW_AT_ONCE: usize = 256;

/// The old place of the move `id`, connected to send the owner libraries.
pub(crate) async fn connected(hub: Arc<Hub>, mut socket: Socket, binding: [u8; 32], id: String, proof: Signature) {
    let mover = hub.store().moves.iter().find(|incoming| incoming.id == id).map(|incoming| incoming.mover);
    let Some(mover) = mover else {
        refuse(&mut socket, Refusal::Broken { reason: format!("the hub came here by no move {id}") }).await;
        return;
    };
    if mover.verify(Domain::Handover, &binding, &proof).is_err() {
        hub.log
            .line(format_args!("refused a connection for the move {id}: the proof of the move's key does not verify"));
        refuse(&mut socket, Refusal::Broken { reason: "the proof of the move's key does not verify".to_string() })
            .await;
        return;
    }
    // What is awaited is of one move: the old place of an earlier one holds
    // libraries as they were before the hub served where that one took it.
    match awaited(&hub).await {
        Ok(coming) if !coming.none() && coming.id != id => {
            let reason = format!("the owner libraries this hub waits for are of the move {}", coming.id);
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
        Ok(_) => {}
        Err(reason) => {
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
    }
    // One at a time: two would write the same library.
    let Ok(_one) = hub.moving.receiving.try_lock() else {
        refuse(&mut socket, Refusal::Broken { reason: "the owner libraries are being sent already".to_string() }).await;
        return;
    };
    if let Err(error) = receive(&hub, &mut socket).await {
        hub.log.line(format_args!("the owner libraries from the old place of the move {id}: {error}"));
    }
    let _ = futures_util::SinkExt::close(&mut socket).await;
}

/// What is still to come to `hub` from where it was.
async fn awaited(hub: &Arc<Hub>) -> Result<super::Coming, String> {
    let hub = hub.clone();
    fastctx_platform::task::spawn_blocking(move || hub.assets.awaited())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("reading which libraries are to come: {error}"))
}

/// Takes what the old place sends of the owner libraries still to come,
/// then of their large objects, until it ends the connection.
async fn receive(hub: &Arc<Hub>, socket: &mut Socket) -> Result<(), String> {
    let dir = hub.store().dir().to_path_buf();
    // A path sent is of a library still to come: one that came is open, and
    // nothing is written under it.
    let within = |path: &str, coming: &super::Coming| -> Result<(), String> {
        match files::library_of(path) {
            Some(library) if coming.libraries.contains(library) => Ok(()),
            _ => Err(format!("{path:?} is of no owner library still to come")),
        }
    };
    // The library whose large objects were last asked after: those sent are
    // its.
    let mut following: Option<String> = None;
    loop {
        let frame = match frames::frame::<_, ToNew>(socket).await {
            Ok(frame) => frame,
            // The old place sent all it had and closed.
            Err(error) if error.contains("closed the connection") => return Ok(()),
            Err(error) => return Err(error),
        };
        match frame {
            ToNew::Coming => {
                let coming = awaited(hub).await?;
                let (libraries, objects) = (coming.libraries.into_iter().collect(), coming.objects.into_iter().collect());
                frames::send(socket, &ToOld::Awaited { libraries, objects }).await?;
            }
            ToNew::Holds { part: Some(Part::Library(library)), .. } => {
                within(&library, &awaited(hub).await?)?;
                let entries = standby::listing(socket, &dir, Some(Part::Library(library))).await?;
                frames::send(socket, &ToOld::Holds { committed: true, sqlite: true, apart: true }).await?;
                frames::send_listing(socket, entries).await?;
            }
            ToNew::Put { path, bytes, packed } => {
                within(&path, &awaited(hub).await?)?;
                standby::receive(socket, &dir, &path, bytes, packed).await?;
            }
            ToNew::Dir { path } => {
                within(&path, &awaited(hub).await?)?;
                let place = files::place(&dir, &path)?;
                let base = dir.clone();
                standby::blocking(move || standby::make_dirs(&base, &place))
                    .await
                    .map_err(|error| format!("making {path}: {error}"))?;
            }
            ToNew::Remove { path } => {
                within(&path, &awaited(hub).await?)?;
                let place = files::place(&dir, &path)?;
                standby::blocking(move || match fastctx_platform::disk::symlink_metadata(&place) {
                    Ok(found) if found.is_dir() => fastctx_platform::disk::remove_dir_all(&place),
                    Ok(_) => fastctx_platform::disk::remove_file(&place),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                })
                .await
                .map_err(|error| format!("removing {path}: {error}"))?;
            }
            ToNew::Arrived { library, digest } => {
                within(&library, &awaited(hub).await?)?;
                let entries = standby::listing(socket, &dir, Some(Part::Library(library.clone()))).await?;
                if files::digest(&entries) != digest {
                    let reason = format!("what this hub holds of {library} is not what the old place sent");
                    frames::send(socket, &ToOld::Refused { reason: reason.clone() }).await?;
                    return Err(reason);
                }
                let (holder, came) = (hub.clone(), library.clone());
                let left = fastctx_platform::task::spawn_blocking(move || holder.assets.arrived(&came))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| format!("opening {library}: {error}"))?;
                hub.log.line(format_args!("the owner library {library} came from where the hub was; {left} still to come"));
                frames::send(socket, &ToOld::Opened).await?;
                // What the hub tells its account, which waited for it: the
                // account as the directory holds it, opened to the
                // nanosecond, which its library's path does not say.
                if let Some(crate::assets::Owner::Account(name, opened)) = crate::assets::Assets::owner_at(&library)
                    && let Ok(name) = name.parse::<fastctx_types::AccountName>()
                    && let Some(opened) = crate::assets::mirror::incarnation(hub, &name, opened)
                {
                    crate::assets::told::caught_up(hub, &name, opened).await;
                }
            }
            ToNew::Follow { library, objects } => {
                let asked: Vec<([u8; 32], u64)> = objects.iter().map(|object| (object.sha256.0, object.bytes)).collect();
                let (holder, path) = (hub.clone(), library.clone());
                let lacking = fastctx_platform::task::spawn_blocking(move || holder.assets.lacking(&path, &asked));
                let lacking = frames::working::<_, ToOld, _>(socket, lacking)
                    .await?
                    .map_err(|error| error.to_string())?
                    .map_err(|error| format!("looking for the large objects of {library}: {error}"))?;
                following = Some(library);
                frames::send(socket, &ToOld::Lacking { objects: lacking }).await?;
            }
            ToNew::Object { sha256, bytes } => {
                let Some(library) = following.as_deref() else {
                    return Err("an object came of no library asked after".to_string());
                };
                take_object(hub, socket, library, sha256.0, bytes).await?;
            }
            ToNew::Followed { library } => {
                let (holder, path) = (hub.clone(), library.clone());
                let left = fastctx_platform::task::spawn_blocking(move || holder.assets.followed(&path))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| format!("noting that the large objects of {library} came: {error}"))?;
                following = None;
                hub.log.line(format_args!(
                    "the large objects of {library} came from where the hub was; {left} librar(ies) still have something to come"
                ));
                frames::send(socket, &ToOld::Kept).await?;
            }
            other => return Err(format!("the old place said {other:?} while sending the owner libraries")),
        }
    }
}

/// The pieces of an object as they come, read by what keeps it.
struct Pieces {
    from: tokio::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
    piece: Vec<u8>,
    at: usize,
}

impl std::io::Read for Pieces {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.piece.len() {
            match self.from.blocking_recv() {
                Some(Ok(piece)) => {
                    self.piece = piece;
                    self.at = 0;
                }
                // Cut short: what came so far is no object.
                Some(Err(error)) => return Err(std::io::Error::other(error)),
                None => return Ok(0),
            }
        }
        let count = buffer.len().min(self.piece.len() - self.at);
        buffer[..count].copy_from_slice(&self.piece[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

/// Takes the `bytes` of the object `sha256` of `library` as they come,
/// kept as they are read: no copy of them is made beside the library.
async fn take_object(hub: &Arc<Hub>, socket: &mut Socket, library: &str, sha256: [u8; 32], bytes: u64) -> Result<(), String> {
    let (pieces, from) = tokio::sync::mpsc::channel::<Result<Vec<u8>, String>>(4);
    let (holder, path) = (hub.clone(), library.to_string());
    let keeping = fastctx_platform::task::spawn_blocking(move || {
        holder.assets.keep_object(&path, &mut Pieces { from, piece: Vec::new(), at: 0 }, &sha256, bytes)
    });
    let mut got = 0;
    let mut fed = Ok(());
    while got < bytes {
        let piece = match frames::recv::<_, ToNew>(socket, true).await {
            Ok(Got::Bytes(piece)) => frames::unpack(&piece),
            Ok(Got::Frame(_)) => Err("an object came with other than its bytes".to_string()),
            Err(error) => Err(error),
        };
        let piece = match piece {
            Ok(piece) if got + piece.len() as u64 <= bytes => piece,
            Ok(_) => {
                fed = Err("an object came with more than its bytes".to_string());
                break;
            }
            Err(error) => {
                fed = Err(error);
                break;
            }
        };
        got += piece.len() as u64;
        // Keeping it ended before its bytes did: why is what it says.
        if pieces.send(Ok(piece)).await.is_err() {
            break;
        }
    }
    if let Err(error) = &fed {
        let _ = pieces.send(Err(error.clone())).await;
    }
    drop(pieces);
    let kept = keeping.await.map_err(|error| error.to_string())?;
    fed?;
    kept.map_err(|error| format!("keeping an object of {library}: {error}"))
}

/// Sends the owner libraries the new hub still waits for, and the large
/// objects of each, from the old place in `dir`, until nothing is left:
/// tried again, a while after each failure, for as long as this place runs.
pub(crate) async fn send(dir: std::path::PathBuf, record: Outgoing, pin: CertPin, log: Arc<Log>) {
    let mut failures = 0;
    let mut said = String::new();
    // How often sending each library failed: those that never did go first.
    let mut troubled: BTreeMap<String, u32> = BTreeMap::new();
    loop {
        match send_all(&dir, &record, pin, &log, &mut troubled).await {
            Ok(()) => {
                log.line(format_args!("every owner library is with the new hub, and its large objects"));
                return;
            }
            Err(reason) => {
                if reason != said {
                    log.line(format_args!("sending the owner libraries to the new hub: {reason}"));
                    said = reason;
                }
                failures += 1;
                tokio::time::sleep(fastctx_net::again::backoff(failures)).await;
            }
        }
    }
}

/// What kept a library from going.
enum Trouble {
    /// It cannot be read here: the connection stands, and the others go.
    Here(String),
    /// The new hub, or the connection to it: nothing more goes on this one.
    There(String),
}

async fn send_all(
    dir: &Path,
    record: &Outgoing,
    pin: CertPin,
    log: &Log,
    troubled: &mut BTreeMap<String, u32>,
) -> Result<(), String> {
    let mut socket = super::out::dial_new(record, pin, HandoverRole::Libraries).await?;
    frames::send(&mut socket, &ToNew::Coming).await?;
    let (libraries, objects) = match frames::frame::<_, ToOld>(&mut socket).await? {
        ToOld::Awaited { libraries, objects } => (libraries, objects),
        ToOld::Refused { reason } => return Err(format!("the new hub refused: {reason}")),
        other => return Err(format!("the new hub answered {other:?}")),
    };
    if libraries.is_empty() && objects.is_empty() {
        let _ = futures_util::SinkExt::close(&mut socket).await;
        return Ok(());
    }
    // The smallest first: the most owners are served again the soonest;
    // after them, those that failed before.
    let every: BTreeSet<String> = libraries.iter().chain(&objects).cloned().collect();
    let sizes: BTreeMap<String, u64> = {
        let (dir, every) = (dir.to_path_buf(), every.clone());
        fastctx_platform::task::spawn_blocking(move || {
            every.into_iter().map(|library| (size(&dir.join(&library)), library)).map(|(size, library)| (library, size)).collect()
        })
        .await
        .map_err(|error| error.to_string())?
    };
    let ordered = |mut some: Vec<String>, troubled: &BTreeMap<String, u32>| {
        some.sort_by_key(|library| (troubled.get(library).copied().unwrap_or(0), sizes.get(library).copied().unwrap_or(0)));
        some
    };
    if !libraries.is_empty() {
        log.line(format_args!("{} owner libraries go to the new hub at {}", libraries.len(), list(&record.addresses)));
    }
    // What never failed goes first, the libraries and then their large
    // objects; what failed before goes after all of that, so one that fails
    // every time — at this end or the other — keeps nothing else waiting.
    let failed_before: BTreeSet<String> =
        troubled.iter().filter(|(_, failures)| **failures > 0).map(|(library, _)| library.clone()).collect();
    let mut left = Vec::new();
    // Libraries whose databases are not with the new hub: their objects wait.
    let mut waiting: BTreeSet<String> = libraries.iter().cloned().collect();
    for again in [false, true] {
        let (some_libraries, some_objects): (Vec<String>, Vec<String>) = (
            libraries.iter().filter(|library| failed_before.contains(*library) == again).cloned().collect(),
            objects.iter().filter(|library| failed_before.contains(*library) == again).cloned().collect(),
        );
        for library in ordered(some_libraries, troubled) {
            match library_goes(&mut socket, dir, &library, log).await {
                Ok(()) => {
                    waiting.remove(&library);
                }
                Err(Trouble::Here(reason)) => {
                    *troubled.entry(library.clone()).or_default() += 1;
                    left.push(format!("{library}: {reason}"));
                }
                Err(Trouble::There(reason)) => {
                    *troubled.entry(library).or_default() += 1;
                    return Err(reason);
                }
            }
        }
        for library in ordered(some_objects, troubled) {
            if waiting.contains(&library) {
                continue;
            }
            match objects_follow(&mut socket, dir, &library, log).await {
                Ok(()) => {}
                Err(Trouble::Here(reason)) => {
                    *troubled.entry(library.clone()).or_default() += 1;
                    left.push(format!("the large objects of {library}: {reason}"));
                }
                Err(Trouble::There(reason)) => {
                    *troubled.entry(library).or_default() += 1;
                    return Err(reason);
                }
            }
        }
    }
    let _ = futures_util::SinkExt::close(&mut socket).await;
    if left.is_empty() { Ok(()) } else { Err(format!("{} could not be sent: {}", left.len(), left.join("; "))) }
}

/// Sends the databases of `library` and has the new hub open it.
async fn library_goes(socket: &mut HubSocket, dir: &Path, library: &str, log: &Log) -> Result<(), Trouble> {
    // One with a database of it set aside, under repair, is not the library
    // it was: sent so, the new hub would make the missing database anew and
    // find the library older than its record of it.
    let whole = {
        let mut place = dir.to_path_buf();
        place.extend(library.split('/'));
        fastctx_platform::task::spawn_blocking(move || fastctx_content::v2::whole(&place)).await.unwrap_or(false)
    };
    if !whole {
        return Err(Trouble::Here("its databases are not both there: it is under repair, or was never whole".to_string()));
    }
    let part = Part::Library(library.to_string());
    let theirs = super::out::holds(socket, Some(part.clone())).await.map_err(Trouble::There)?;
    // The closed copies of its databases, made while the new hub is told
    // this end works.
    let listed = {
        let (dir, part) = (dir.to_path_buf(), part);
        fastctx_platform::task::spawn_blocking(move || files::snapshot(&dir, &part))
    };
    let listed = frames::working::<_, ToNew, _>(socket, listed).await.map_err(Trouble::There)?;
    let clear = || {
        let (place, under) = (dir.to_path_buf(), library.to_string());
        fastctx_platform::task::spawn_blocking(move || super::snapshot::clear(&place, Some(&under)))
    };
    let mine = match listed {
        Ok(Ok(mine)) => mine,
        Ok(Err(error)) => {
            let _ = clear().await;
            return Err(Trouble::Here(format!("its databases cannot be read: {error}")));
        }
        Err(error) => {
            let _ = clear().await;
            return Err(Trouble::Here(error.to_string()));
        }
    };
    let sent = super::out::send_difference(socket, dir, &mine, &theirs, false).await;
    // The copies are sent, or made again.
    let _ = clear().await;
    let sent = sent.map_err(Trouble::There)?;
    frames::send(socket, &ToNew::Arrived { library: library.to_string(), digest: files::digest(&mine) })
        .await
        .map_err(Trouble::There)?;
    match frames::frame::<_, ToOld>(socket).await.map_err(Trouble::There)? {
        ToOld::Opened => {
            log.line(format_args!("the owner library {library} is with the new hub: {} bytes sent now", sent.bytes));
            Ok(())
        }
        ToOld::Refused { reason } => Err(Trouble::There(format!("the new hub did not take {library}: {reason}"))),
        other => Err(Trouble::There(format!("the new hub answered {other:?}"))),
    }
}

/// Sends the large objects of `library` the new hub does not hold.
async fn objects_follow(socket: &mut HubSocket, dir: &Path, library: &str, log: &Log) -> Result<(), Trouble> {
    let listed = {
        let (dir, library) = (dir.to_path_buf(), library.to_string());
        fastctx_platform::task::spawn_blocking(move || super::snapshot::following(&dir, &library))
    };
    let objects = match frames::working::<_, ToNew, _>(socket, listed).await.map_err(Trouble::There)? {
        Ok(Ok(objects)) => objects,
        Ok(Err(error)) => return Err(Trouble::Here(format!("which follow it cannot be read: {error}"))),
        Err(error) => return Err(Trouble::Here(error.to_string())),
    };
    let (mut sent, mut bytes, mut missing) = (0u64, 0u64, 0u64);
    for batch in objects.chunks(FOLLOW_AT_ONCE) {
        let asked = batch.iter().map(|(hash, size, _)| frames::Object { sha256: Digest(*hash), bytes: *size }).collect();
        frames::send(socket, &ToNew::Follow { library: library.to_string(), objects: asked }).await.map_err(Trouble::There)?;
        let lacking = match frames::frame::<_, ToOld>(socket).await.map_err(Trouble::There)? {
            ToOld::Lacking { objects } => objects,
            ToOld::Refused { reason } => return Err(Trouble::There(format!("the new hub refused: {reason}"))),
            other => return Err(Trouble::There(format!("the new hub answered {other:?}"))),
        };
        for at in lacking {
            let Some((hash, size, file)) = batch.get(at as usize) else {
                return Err(Trouble::There("the new hub named an object it was not asked after".to_string()));
            };
            if send_object(socket, hash, *size, file).await? {
                sent += 1;
                bytes += size;
            } else {
                missing += 1;
            }
        }
    }
    frames::send(socket, &ToNew::Followed { library: library.to_string() }).await.map_err(Trouble::There)?;
    match frames::frame::<_, ToOld>(socket).await.map_err(Trouble::There)? {
        ToOld::Kept => {}
        ToOld::Refused { reason } => return Err(Trouble::There(format!("the new hub refused: {reason}"))),
        other => return Err(Trouble::There(format!("the new hub answered {other:?}"))),
    }
    log.line(format_args!(
        "the {} large objects of {library} are with the new hub: {sent} sent now, {bytes} bytes",
        objects.len() as u64 - missing
    ));
    if missing > 0 {
        log.line(format_args!(
            "{missing} large objects of {library} are not here as their rows say — no file, or one that is not the object: the hub held none of them to send"
        ));
    }
    Ok(())
}

/// Sends the object `hash` from `file`, a piece at a time. Whether it went:
/// one whose file is not there, or is not that object — another length,
/// other bytes — the hub did not hold, and none of it is sent. A file that
/// is there and cannot be read now is this library's trouble: its objects
/// are not said to have come, and are sent at a later round.
async fn send_object(socket: &mut HubSocket, hash: &[u8; 32], size: u64, file: &Path) -> Result<bool, Trouble> {
    use sha2::Digest as _;
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let unread = |error: std::io::Error| Trouble::Here(format!("{} cannot be read: {error}", file.display()));
    let mut source = match tokio::fs::File::open(file).await {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(unread(error)),
    };
    if source.metadata().await.map_err(unread)?.len() != size {
        return Ok(false);
    }
    // Read whole before any of it goes: bytes that are not the object's
    // would be refused where they came, and end the sending there.
    let mut hasher = sha2::Sha256::new();
    let mut piece = vec![0; frames::CHUNK];
    loop {
        let read = source.read(&mut piece).await.map_err(unread)?;
        if read == 0 {
            break;
        }
        hasher.update(&piece[..read]);
    }
    if <[u8; 32]>::from(hasher.finalize()) != *hash {
        return Ok(false);
    }
    source.seek(std::io::SeekFrom::Start(0)).await.map_err(unread)?;
    frames::send(socket, &ToNew::Object { sha256: Digest(*hash), bytes: size }).await.map_err(Trouble::There)?;
    let mut left = size;
    while left > 0 {
        let mut piece = vec![0; usize::try_from(left).unwrap_or(usize::MAX).min(frames::CHUNK)];
        // Cut short once its bytes go, the connection cannot go on.
        source.read_exact(&mut piece).await.map_err(|error| Trouble::There(format!("reading a large object: {error}")))?;
        left -= piece.len() as u64;
        frames::send_bytes(socket, frames::pack(piece)).await.map_err(Trouble::There)?;
    }
    Ok(true)
}

/// How many bytes the databases of the library at `path` come to: the files
/// in its directory, not those below.
fn size(path: &Path) -> u64 {
    let Ok(entries) = fastctx_platform::disk::read_dir(path) else { return 0 };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.metadata().map_or(0, |found| found.len()))
        .sum()
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
