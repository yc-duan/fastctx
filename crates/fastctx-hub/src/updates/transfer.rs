//! The hub as one end of a package's data connection: handing a package it
//! holds to a member (`Intent::Package`), and taking one a member fetched
//! for it (`Intent::Deliver`). The hub keeps such a connection alive as
//! either end of a relayed one does: something of its own at least every
//! heartbeat, and the member taken for gone once nothing of its own has come
//! for the silence.

use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use fastctx_net::link::versions::Sized;
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, HEARTBEAT, Refusal, SILENCE};
use fastctx_types::update::{Held, Manifest};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{Hub, Socket, refuse, send};

/// What the member says once it has every byte.
const DONE: &str = "\"done\"";
/// The most bytes one message carries.
const CHUNK: usize = 64 * 1024;

/// Hands the member the package `target` of `version` from the byte `from`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    hub: Arc<Hub>,
    mut socket: Socket,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    version: String,
    target: String,
    from: u64,
) {
    let me = match hub.member(&key, &proof, &binding) {
        Ok(me) => me,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let cache = hub.updates.lock().expect("never poisoned").cache.clone();
    let Some(path) = cache.path(&version, &target).ok().filter(|path| path.is_file()) else {
        let reason = format!("the hub does not hold {target} of {version}");
        refuse(&mut socket, Refusal::Broken { reason }).await;
        return;
    };
    let opened = std::fs::File::open(&path).and_then(|mut file| {
        let size = file.metadata()?.len();
        file.seek(std::io::SeekFrom::Start(from.min(size)))?;
        Ok((file, size))
    });
    let (mut file, size) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            let reason = format!("reading {target} of {version}: {error}");
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
    };
    if send(&mut socket, &Down::Paired).await.is_err() {
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let writer = async {
        let said = serde_json::to_string(&Sized { size }).expect("a size serializes");
        sink.send(WsMessage::Text(said.into())).await.map_err(|error| error.to_string())?;
        let mut buffer = vec![0; CHUNK];
        loop {
            let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            sink.send(WsMessage::Binary(buffer[..read].to_vec().into())).await.map_err(|error| error.to_string())?;
        }
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            sink.send(WsMessage::Ping(Vec::new().into())).await.map_err(|error| error.to_string())?;
        }
    };
    let reader = async {
        loop {
            match tokio::time::timeout(SILENCE, stream.next()).await {
                Ok(Some(Ok(WsMessage::Text(text)))) if text.as_str() == DONE => return Ok(()),
                Ok(Some(Ok(WsMessage::Close(_))) | None) => return Err("the member left".to_string()),
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(error))) => return Err(error.to_string()),
                Err(_) => return Err(format!("nothing came from the member for {} s", SILENCE.as_secs())),
            }
        }
    };
    let ended: Result<(), String> = tokio::select! {
        ended = reader => ended,
        ended = writer => ended,
    };
    match ended {
        Ok(()) => hub.log.line(format_args!("{me} took {target} of {version} from the hub")),
        Err(why) => hub.log.line(format_args!("{me} did not take {target} of {version}: {why}")),
    }
}

/// Takes the package of the request `id` from the member it was asked of.
pub(crate) async fn deliver(
    hub: Arc<Hub>,
    mut socket: Socket,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    id: u64,
) {
    let me = match hub.member(&key, &proof, &binding) {
        Ok(me) => me,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let asked = {
        let updates = hub.updates.lock().expect("never poisoned");
        updates.fetches.get(&id).filter(|fetching| fetching.helper == me).map(|fetching| fetching.held.clone())
    };
    let Some(held) = asked else {
        let reason = format!("the hub asked {me} for no package numbered {id}");
        refuse(&mut socket, Refusal::Broken { reason }).await;
        return;
    };
    let outcome = take(&hub, socket, &held).await;
    if let Err(why) = &outcome {
        hub.log.line(format_args!("taking {} of {} from {me}: {why}", held.target, held.version));
    }
    if let Some(done) =
        hub.updates.lock().expect("never poisoned").fetches.get_mut(&id).and_then(|fetching| fetching.done.take())
    {
        let _ = done.send(outcome);
    }
}

async fn take(hub: &Arc<Hub>, mut socket: Socket, held: &Held) -> Result<(), String> {
    let (manifest, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(&held.version), updates.cache.clone())
    };
    let manifest = manifest.ok_or_else(|| format!("the hub no longer knows the release {}", held.version))?;
    let incoming = cache.incoming(&held.version, &held.target)?;
    send(&mut socket, &Down::Paired).await?;
    let (mut sink, mut stream) = socket.split();
    let received = async {
        let mut file = std::fs::File::create(&incoming).map_err(|error| format!("{}: {error}", incoming.display()))?;
        let mut size = None;
        let mut have = 0_u64;
        while size.is_none_or(|size| have < size) {
            match tokio::time::timeout(SILENCE, stream.next()).await {
                Ok(Some(Ok(WsMessage::Text(text)))) if size.is_none() => {
                    let said: Sized = serde_json::from_str(&text).map_err(|error| error.to_string())?;
                    size = Some(said.size);
                }
                Ok(Some(Ok(WsMessage::Binary(bytes)))) if size.is_some() => {
                    file.write_all(&bytes).map_err(|error| error.to_string())?;
                    have += bytes.len() as u64;
                }
                Ok(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => {}
                Ok(Some(Ok(WsMessage::Close(_))) | None) => {
                    return Err("the member left before it sent it all".to_string());
                }
                Ok(Some(Ok(_))) => return Err("the member sent something other than the package".to_string()),
                Ok(Some(Err(error))) => return Err(error.to_string()),
                Err(_) => return Err(format!("nothing came from the member for {} s", SILENCE.as_secs())),
            }
        }
        if have != size.unwrap_or_default() {
            return Err("the member sent more than it said".to_string());
        }
        file.sync_all().map_err(|error| error.to_string())
    };
    let pinging = async {
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            if sink.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    };
    let received: Result<(), String> = tokio::select! {
        received = received => received,
        () = pinging => unreachable!("pinging never ends"),
    };
    let kept = match received {
        Ok(()) => take_in(hub, &cache, &manifest, &held.target, &incoming).await.map(|_| ()),
        Err(why) => {
            let _ = std::fs::remove_file(&incoming);
            Err(why)
        }
    };
    let word = match &kept {
        Ok(()) => DONE.to_string(),
        Err(why) => serde_json::to_string(&format!("the hub did not take it: {why}")).expect("text serializes"),
    };
    let _ = sink.send(WsMessage::Text(word.into())).await;
    let _ = sink.close().await;
    kept
}

/// Checks the package that came to `incoming` and keeps it, telling every
/// machine the hub holds it; its size.
pub(super) async fn take_in(
    hub: &Arc<Hub>,
    cache: &fastctx_update::Cache,
    manifest: &Manifest,
    target: &str,
    incoming: &Path,
) -> Result<u64, String> {
    let package = manifest.package(target).ok_or("the release has no such package")?;
    let (path, signature, version) = (incoming.to_path_buf(), package.signature.clone(), manifest.version.clone());
    let checked = tokio::task::spawn_blocking(move || fastctx_update::verify::file(&path, &signature, &version)).await;
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(why)) => {
            let _ = std::fs::remove_file(incoming);
            return Err(why);
        }
        Err(error) => return Err(error.to_string()),
    }
    cache.keep_manifest(manifest)?;
    let path = cache.keep(&manifest.version, target, incoming)?;
    let size = std::fs::metadata(&path).map_err(|error| error.to_string())?.len();
    hub.log.line(format_args!("holds {target} of {} now, {size} bytes", manifest.version));
    super::prune(hub, &manifest.version);
    super::tell(hub, None);
    Ok(size)
}
