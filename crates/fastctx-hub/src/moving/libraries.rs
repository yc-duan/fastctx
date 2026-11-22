//! The owner libraries after the switch: sent by the old place, one at a
//! time, to the hub that now serves.
//!
//! A move stops the hub for its core alone (`files::Part`). The owner
//! libraries, the accounts' and the bodies', stay where the hub was until
//! it serves no more: from then nothing writes them, so each is sent once
//! and is what it was. The old place asks the new hub which are still to
//! come, sends the smallest first — its objects went before the switch, so
//! what goes is mostly its databases, written out with only what they hold
//! — and says it came; the new hub checks what it holds of it against the
//! digest of what was sent, and opens it. Until then the new hub opens no
//! library of that owner, nor makes one (`assets::owners`): a request for
//! it fails and is asked again, as one a busy hub did not answer is. What
//! the hub's own records hold — who is a member, which logins assist whom
//! and with what allowance — came with its core; conversations are kept and
//! read on their machines; what waits for a library is what the hub keeps a
//! copy of for the other devices of an account.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fastctx_net::ids::CertPin;
use fastctx_net::moving::HandoverRole;
use fastctx_net::sign::{Domain, Signature};
use fastctx_net::wire::Refusal;

use super::files::{self, Part};
use super::frames::{self, ToNew, ToOld};
use super::{Outgoing, standby};
use crate::log::Log;
use crate::{Hub, Socket, refuse};

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

/// Takes what the old place sends of the owner libraries still to come,
/// until it ends the connection.
async fn receive(hub: &Arc<Hub>, socket: &mut Socket) -> Result<(), String> {
    let dir = hub.store().dir().to_path_buf();
    let awaited = |hub: &Arc<Hub>| {
        let hub = hub.clone();
        async move {
            fastctx_platform::task::spawn_blocking(move || hub.assets.coming())
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("reading which libraries are to come: {error}"))
        }
    };
    // A path sent is of a library still to come: one that came is open, and
    // nothing is written under it.
    let within = |path: &str, coming: &[String]| -> Result<(), String> {
        match files::library_of(path) {
            Some(library) if coming.iter().any(|one| one == library) => Ok(()),
            _ => Err(format!("{path:?} is of no owner library still to come")),
        }
    };
    loop {
        let frame = match frames::frame::<_, ToNew>(socket).await {
            Ok(frame) => frame,
            // The old place sent all it had and closed.
            Err(error) if error.contains("closed the connection") => return Ok(()),
            Err(error) => return Err(error),
        };
        match frame {
            ToNew::Coming => {
                let libraries = awaited(hub).await?;
                frames::send(socket, &ToOld::Awaited { libraries }).await?;
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
                // What the hub tells its account, which waited for it.
                if let Some(crate::assets::Owner::Account(name, opened)) = crate::assets::Assets::owner_at(&library)
                    && let (Ok(name), Ok(opened)) =
                        (name.parse::<fastctx_types::AccountName>(), jiff::Timestamp::from_millisecond(opened))
                {
                    crate::assets::told::caught_up(hub, &name, opened).await;
                }
            }
            other => return Err(format!("the old place said {other:?} while sending the owner libraries")),
        }
    }
}

/// Sends the owner libraries the new hub still waits for, from the old
/// place in `dir`, until none is left: tried again, a while after each
/// failure, for as long as this place runs.
pub(crate) async fn send(dir: std::path::PathBuf, record: Outgoing, pin: CertPin, log: Arc<Log>) {
    let mut failures = 0;
    let mut said = String::new();
    loop {
        match send_all(&dir, &record, pin, &log).await {
            Ok(()) => {
                log.line(format_args!("every owner library is with the new hub"));
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

async fn send_all(dir: &Path, record: &Outgoing, pin: CertPin, log: &Log) -> Result<(), String> {
    let mut socket = super::out::dial_new(record, pin, HandoverRole::Libraries).await?;
    frames::send(&mut socket, &ToNew::Coming).await?;
    let mut awaited = match frames::frame::<_, ToOld>(&mut socket).await? {
        ToOld::Awaited { libraries } => libraries,
        ToOld::Refused { reason } => return Err(format!("the new hub refused: {reason}")),
        other => return Err(format!("the new hub answered {other:?}")),
    };
    if awaited.is_empty() {
        return Ok(());
    }
    // The smallest first: the most owners are served again the soonest.
    let sizes = {
        let (dir, awaited) = (dir.to_path_buf(), awaited.clone());
        fastctx_platform::task::spawn_blocking(move || awaited.iter().map(|library| size(&dir.join(library))).collect::<Vec<u64>>())
            .await
            .map_err(|error| error.to_string())?
    };
    let mut order: Vec<usize> = (0..awaited.len()).collect();
    order.sort_by_key(|&at| sizes[at]);
    awaited = order.into_iter().map(|at| awaited[at].clone()).collect();
    log.line(format_args!("{} owner libraries go to the new hub at {}", awaited.len(), list(&record.addresses)));
    for library in awaited {
        let part = Part::Library(library.clone());
        let theirs = super::out::holds(&mut socket, Some(part.clone())).await?;
        let mine = super::out::listing(&mut socket, dir, part).await?;
        let sent = super::out::send_difference(&mut socket, dir, &mine, &theirs).await;
        // The closed copies of its databases are sent, or made again.
        let (place, under) = (dir.to_path_buf(), library.clone());
        let _ = fastctx_platform::task::spawn_blocking(move || super::snapshot::clear(&place, Some(&under))).await;
        sent?;
        frames::send(&mut socket, &ToNew::Arrived { library: library.clone(), digest: files::digest(&mine) }).await?;
        match frames::frame::<_, ToOld>(&mut socket).await? {
            ToOld::Opened => log.line(format_args!("the owner library {library} is with the new hub")),
            ToOld::Refused { reason } => return Err(format!("the new hub did not take {library}: {reason}")),
            other => return Err(format!("the new hub answered {other:?}")),
        }
    }
    let _ = futures_util::SinkExt::close(&mut socket).await;
    Ok(())
}

/// How many bytes the files under `path` come to.
fn size(path: &Path) -> u64 {
    let Ok(entries) = fastctx_platform::disk::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |found| found.len()),
            Err(_) => 0,
        })
        .sum()
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
