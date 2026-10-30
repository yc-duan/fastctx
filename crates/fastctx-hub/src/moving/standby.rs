//! The new place of a move, until the old hub confirms the switch.
//!
//! The machine the hub moves to makes the new directory from the identity
//! the hub gave it (`prepare`), and the hub's system service runs it from
//! then on. Standing by, it listens at its new address with the hub's own
//! certificate, so the old hub reaching it proves both that it is reachable
//! there and that it is this hub; it takes nothing but the old hub, which
//! proves it holds the move's key. Over that connection the old hub
//! replaces the records here until they are its own, and says when to take
//! over (`Commit`): only when what is here has the digest of what the old
//! hub held when it stopped. Taken over, it still serves nobody until the
//! old hub confirms it recorded the switch, so the two never both serve.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_net::moving::{HandoverRole, Identity};
use fastctx_net::sign::Domain;
use fastctx_net::wire::{self, Intent, Refusal, Up};
use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::MachineName;
use fastctx_types::hub_move::OldPlace;
use jiff::Timestamp;
use tokio::sync::oneshot;

use super::files::{self, Entry};
use super::frames::{self, Got, ToNew, ToOld};
use super::{ARRIVING, Arriving, Incoming, MOVES};
use crate::store::{HubFile, Store, json, read, read_or_default};
use crate::{Kept, Leave, Next, Socket, log::Log, refuse};

/// Makes the directory of a hub arriving here from `identity`, for the move
/// `id` to the machine `to`: the hub's identity, its settings with its new
/// addresses, and the record of the move. A directory made for the same
/// move before is made again, keeping how far the move got there; one that
/// holds another hub, or this hub's old data, is refused. The service that
/// runs it is the caller's.
pub fn prepare(dir: &Path, id: &str, to: &MachineName, identity: &Identity) -> anyhow::Result<()> {
    let found = arriving_in(dir)?;
    if let Some(found) = &found {
        if found.id != id {
            anyhow::bail!("{} holds a hub arriving by another move", dir.display());
        }
    } else if Store::hub_file(dir).exists() {
        anyhow::bail!("{} holds a hub already: this hub's old data, or another network's hub", dir.display());
    } else if super::retired(dir)? {
        std::fs::remove_file(dir.join(super::RETIRED))?;
    } else if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            if !entry?.file_name().to_string_lossy().starts_with("hub.log") {
                anyhow::bail!("{} is not empty", dir.display());
            }
        }
    }
    let port =
        identity.addresses.first().map(SocketAddr::port).ok_or_else(|| anyhow::anyhow!("the move gives no address"))?;
    // Every address loopback: nothing beyond this machine is to reach it.
    let first = identity.addresses[0].ip();
    let ip = match (identity.addresses.iter().all(|address| address.ip().is_loopback()), first) {
        (true, IpAddr::V4(_)) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        (true, IpAddr::V6(_)) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        (false, IpAddr::V4(_)) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        (false, IpAddr::V6(_)) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let listen = SocketAddr::new(ip, port);
    // A port taken here would keep the hub's service failing to start; a
    // directory made before for this move may have its service on it already.
    if found.is_none()
        && let Err(error) = std::net::TcpListener::bind(listen)
    {
        anyhow::bail!("the hub cannot listen on {listen} here: {error}");
    }
    let hub = HubFile {
        id: identity.id.clone(),
        name: identity.name.clone(),
        public: identity.addresses.clone(),
        listen,
        self_registration: identity.self_registration,
        dissolved: None,
        updates: Default::default(),
        due: Vec::new(),
        repairs: Vec::new(),
        package_targets: Default::default(),
    };
    let arriving = Arriving {
        id: id.to_string(),
        mover: identity.mover,
        to: to.clone(),
        addresses: identity.addresses.clone(),
        from: identity.from.clone(),
        committed: found.is_some_and(|found| found.committed),
    };
    ensure_private_dir(dir)?;
    ensure_private_dir(&dir.join("tls"))?;
    publish(&Store::certificate_file(dir), &identity.certificate.0, Permissions::Private)?;
    publish(&Store::key_file(dir), &identity.key.0, Permissions::Private)?;
    publish(&Store::secrets_file(dir), &identity.secrets.0, Permissions::Private)?;
    publish(&dir.join(ARRIVING), &json(&arriving)?, Permissions::Private)?;
    // Last: a directory with `hub.json` is a hub.
    publish(&Store::hub_file(dir), &json(&hub)?, Permissions::Private)?;
    Ok(())
}

/// The move arriving in `dir`, if one is.
pub fn arriving_in(dir: &Path) -> std::io::Result<Option<Arriving>> {
    match read::<Arriving>(&dir.join(ARRIVING)) {
        Ok(arriving) => Ok(Some(arriving)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The new place standing by.
pub(crate) struct Standby {
    dir: PathBuf,
    log: Arc<Log>,
    arriving: Mutex<Arriving>,
    /// One handover at a time.
    syncing: tokio::sync::Mutex<()>,
    done: Mutex<Option<oneshot::Sender<Done>>>,
}

enum Done {
    /// The old hub confirmed the switch: serve.
    Serve,
    /// The move was called off.
    CalledOff,
}

impl Standby {
    /// Where the standby writes what happens to it.
    pub(crate) fn log(&self) -> &Log {
        &self.log
    }

    fn arriving(&self) -> Arriving {
        self.arriving.lock().expect("never poisoned").clone()
    }

    fn finish(&self, done: Done) {
        if let Some(sender) = self.done.lock().expect("never poisoned").take() {
            let _ = sender.send(done);
        }
    }
}

/// Stands by in `dir` until the old hub confirms the switch, then serves.
pub(crate) async fn run(
    dir: &Path,
    kept: Option<Kept>,
    arriving: Arriving,
    log: Arc<Log>,
    leave: &Leave,
) -> anyhow::Result<Next> {
    let hub: HubFile = read(&Store::hub_file(dir))?;
    let certificate = fastctx_net::tls::HubCertificate {
        certificate: std::fs::read(Store::certificate_file(dir))?,
        key: std::fs::read(Store::key_file(dir))?,
    };
    let tls = crate::releases::http::Tls::of(&certificate).map_err(anyhow::Error::msg)?;
    let kept = crate::bind(kept, hub.listen).await?;
    log.line(format_args!(
        "hub {} ({}) {} stands by on {} to take over from {} by the move {}; certificate {}",
        hub.name,
        hub.id,
        env!("CARGO_PKG_VERSION"),
        hub.listen,
        list(&arriving.from),
        arriving.id,
        certificate.pin()
    ));
    let (done, mut finished) = oneshot::channel();
    let standby = Arc::new(Standby {
        dir: dir.to_path_buf(),
        log: log.clone(),
        arriving: Mutex::new(arriving),
        syncing: tokio::sync::Mutex::new(()),
        done: Mutex::new(Some(done)),
    });
    loop {
        tokio::select! {
            accepted = kept.listener.accept() => match accepted {
                Ok((tcp, peer)) => {
                    fastctx_net::dial::configure(&tcp);
                    let host = crate::releases::http::Host::Standby(standby.clone());
                    tokio::spawn(crate::releases::http::serve(host, tls.clone(), tcp, peer));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    log.line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            () = crate::shutdown_signal() => {
                log.line(format_args!("shutting down"));
                return Ok(Next::Stop);
            }
            done = &mut finished => match done {
                Ok(Done::Serve) => {
                    log.line(format_args!("the old hub confirmed the switch: this hub serves from now on"));
                    return Ok(Next::Again(Some(kept)));
                }
                Ok(Done::CalledOff) | Err(_) => {
                    drop(kept);
                    super::retire(dir, "the move that was to bring the hub here was called off", &log, leave);
                    return Ok(Next::Again(None));
                }
            },
        }
    }
}

pub(crate) async fn connected(
    standby: Arc<Standby>,
    mut socket: Socket,
    path: String,
    binding: [u8; 32],
    hello: Up,
    peer: SocketAddr,
) {
    let refusal = "this hub stands by to take over from another and serves nobody yet".to_string();
    let (id, role, proof) = match (path.as_str(), hello) {
        (wire::CONTROL_PATH, Up::Hello { intent: Intent::Handover { id, role, proof }, .. }) => (id, role, proof),
        _ => {
            refuse(&mut socket, Refusal::Broken { reason: refusal }).await;
            return;
        }
    };
    let arriving = standby.arriving();
    if id != arriving.id {
        refuse(&mut socket, Refusal::Broken { reason: super::NOT_ARRIVING.to_string() }).await;
        return;
    }
    if arriving.mover.verify(Domain::Handover, &binding, &proof).is_err() {
        standby.log.line(format_args!("{peer}: refused, the proof of the move's key does not verify"));
        refuse(&mut socket, Refusal::Broken { reason: "the proof of the move's key does not verify".to_string() })
            .await;
        return;
    }
    match role {
        HandoverRole::Sync => {
            let Ok(_one) = standby.syncing.try_lock() else {
                refuse(&mut socket, Refusal::Broken { reason: "another handover is under way".to_string() }).await;
                return;
            };
            standby.log.line(format_args!("{peer}: the old hub hands over"));
            match sync(&standby, &mut socket).await {
                Ok(()) => {}
                Err(error) => standby.log.line(format_args!("{peer}: the handover ended: {error}")),
            }
            let _ = socket.close(None).await;
        }
        // The old place, forwarding: it recorded the switch, though its
        // confirmation did not come here.
        HandoverRole::Old if arriving.committed => {
            standby.log.line(format_args!("{peer}: the old place connected; it recorded the switch"));
            match confirm(&standby) {
                Ok(()) => {
                    refuse(&mut socket, Refusal::Broken { reason: "this hub serves now; connect again".to_string() })
                        .await;
                    standby.finish(Done::Serve);
                }
                Err(error) => {
                    refuse(&mut socket, Refusal::Broken { reason: error }).await;
                }
            }
        }
        HandoverRole::Old => {
            refuse(&mut socket, Refusal::Broken { reason: "this place has not taken the hub's records over".into() })
                .await;
        }
    }
}

/// Answers the old hub until it confirms the switch or calls the move off.
async fn sync(standby: &Standby, socket: &mut Socket) -> Result<(), String> {
    let dir = standby.dir.clone();
    loop {
        match frames::frame::<_, ToNew>(socket).await? {
            ToNew::Holds { sqlite } => {
                if !sqlite {
                    let reason =
                        "update the sending hub before moving: this version does not transfer all hub settings"
                            .to_string();
                    frames::send(socket, &ToOld::Refused { reason: reason.clone() }).await?;
                    return Err(reason);
                }
                let entries = listing(socket, &dir).await?;
                frames::send(socket, &ToOld::Holds { committed: standby.arriving().committed, sqlite: true }).await?;
                frames::send_listing(socket, entries).await?;
            }
            ToNew::Put { path, bytes } => receive(socket, &dir, &path, bytes).await?,
            ToNew::Dir { path } => {
                let place = files::place(&dir, &path)?;
                let base = dir.clone();
                blocking(move || make_dirs(&base, &place)).await.map_err(|error| format!("making {path}: {error}"))?;
            }
            ToNew::Remove { path } => {
                let place = files::place(&dir, &path)?;
                blocking(move || match std::fs::symlink_metadata(&place) {
                    Ok(found) if found.is_dir() => std::fs::remove_dir_all(&place),
                    Ok(_) => std::fs::remove_file(&place),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                })
                .await
                .map_err(|error| format!("removing {path}: {error}"))?;
            }
            ToNew::Check => {
                let entries = listing(socket, &dir).await?;
                frames::send(socket, &ToOld::Checked { digest: files::digest(&entries) }).await?;
            }
            ToNew::Commit { digest, self_registration, record } => {
                let entries = listing(socket, &dir).await?;
                if files::digest(&entries) != digest {
                    let reason = "what this place holds is not what the old hub sent".to_string();
                    frames::send(socket, &ToOld::Refused { reason: reason.clone() }).await?;
                    return Err(reason);
                }
                if let Err(error) = commit(standby, self_registration, &record) {
                    frames::send(socket, &ToOld::Refused { reason: error.clone() }).await?;
                    return Err(error);
                }
                standby.log.line(format_args!("took the hub's records over; waiting for the old hub to confirm"));
                frames::send(socket, &ToOld::Committed).await?;
            }
            ToNew::Confirm => {
                if let Err(error) = confirm(standby) {
                    frames::send(socket, &ToOld::Refused { reason: error.clone() }).await?;
                    return Err(error);
                }
                frames::send(socket, &ToOld::Confirmed).await?;
                standby.finish(Done::Serve);
                return Ok(());
            }
            ToNew::Cancel => {
                standby.log.line(format_args!("the old hub called the move off"));
                frames::send(socket, &ToOld::Cancelled).await?;
                standby.finish(Done::CalledOff);
                return Ok(());
            }
            other => return Err(format!("the old hub said {other:?} while handing over")),
        }
    }
}

/// Takes the file `path` of `bytes`, a piece at a time, beside where it
/// goes; it replaces what is there only when its digest is the one the old
/// hub says.
async fn receive(socket: &mut Socket, dir: &Path, path: &str, bytes: u64) -> Result<(), String> {
    use sha2::Digest as _;
    let place = files::place(dir, path)?;
    let (base, target) = (dir.to_path_buf(), place.clone());
    let mut staged = blocking(move || {
        make_parents(&base, &target)?;
        fastctx_platform::Staged::create(&target, cfg!(unix).then_some(0o600))
    })
    .await
    .map_err(|error| format!("writing {path}: {error}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut got = 0;
    while got < bytes {
        let piece = match frames::recv::<_, ToNew>(socket, true).await? {
            Got::Bytes(piece) if got + piece.len() as u64 <= bytes => piece,
            _ => return Err(format!("{path} came with more or other than its {bytes} bytes")),
        };
        got += piece.len() as u64;
        hasher.update(&piece);
        staged = blocking(move || staged.write_all(&piece).map(|()| staged))
            .await
            .map_err(|error| format!("writing {path}: {error}"))?;
    }
    match frames::frame::<_, ToNew>(socket).await? {
        ToNew::Sum { sha256 } if sha256.0 == <[u8; 32]>::from(hasher.finalize()) => {}
        ToNew::Sum { .. } => return Err(format!("{path} came with other bytes than its digest says")),
        other => return Err(format!("the old hub said {other:?} after the bytes of {path}")),
    }
    blocking(move || staged.commit()).await.map_err(|error| format!("writing {path}: {error}"))
}

/// What is here, listed while the old hub is told this end is working.
async fn listing(socket: &mut Socket, dir: &Path) -> Result<Vec<Entry>, String> {
    let dir = dir.to_path_buf();
    frames::working::<_, ToOld, _>(socket, blocking(move || files::manifest(&dir)))
        .await?
        .map_err(|error| format!("listing the records: {error}"))
}

/// Takes the records over: the move joins the moves that brought the hub
/// here, with every member there is now, and the hub's settings take the
/// old hub's last.
fn commit(standby: &Standby, self_registration: bool, record: &frames::Record) -> Result<(), String> {
    let dir = &standby.dir;
    let mut arriving = standby.arriving();
    let directory = Store::read_directory(dir).map_err(|error| format!("reading the directory: {error}"))?;
    let members = directory.members.iter().map(|member| (member.name.clone(), member.is_machine())).collect();
    let mut moves: Vec<Incoming> =
        read_or_default(&dir.join(MOVES)).map_err(|error| format!("reading the moves: {error}"))?;
    moves.retain(|incoming| incoming.id != arriving.id);
    moves.push(Incoming {
        id: arriving.id.clone(),
        to: arriving.to.clone(),
        addresses: arriving.addresses.clone(),
        from: arriving.from.clone(),
        by: record.by.clone(),
        started: record.started,
        switched: Timestamp::now(),
        mover: arriving.mover,
        members,
        followed: Default::default(),
        old: OldPlace::Forwarding,
    });
    let written = (|| {
        publish(&dir.join(MOVES), &json(&moves)?, Permissions::Private)?;
        let mut hub: HubFile = read(&Store::hub_file(dir))?;
        hub.self_registration = self_registration;
        publish(&Store::hub_file(dir), &json(&hub)?, Permissions::Private)?;
        arriving.committed = true;
        publish(&dir.join(ARRIVING), &json(&arriving)?, Permissions::Private)
    })();
    written.map_err(|error| format!("taking the records over: {error}"))?;
    *standby.arriving.lock().expect("never poisoned") = arriving;
    Ok(())
}

/// The old hub recorded the switch: nothing stands by any more.
fn confirm(standby: &Standby) -> Result<(), String> {
    if !standby.arriving().committed {
        return Err("this place has not taken the hub's records over".to_string());
    }
    match std::fs::remove_file(standby.dir.join(ARRIVING)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("recording the switch: {error}")),
        _ => Ok(()),
    }
}

/// Makes the directories between `base` and the file `place`, private.
fn make_parents(base: &Path, place: &Path) -> std::io::Result<()> {
    match place.parent() {
        Some(parent) => make_dirs(base, parent),
        None => Ok(()),
    }
}

/// Makes the directory `place` under `base` and those between, private.
fn make_dirs(base: &Path, place: &Path) -> std::io::Result<()> {
    let relative = place.strip_prefix(base).map_err(std::io::Error::other)?;
    let mut at = base.to_path_buf();
    for name in relative.components() {
        at.push(name);
        ensure_private_dir(&at)?;
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(std::io::Error::other)?
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
