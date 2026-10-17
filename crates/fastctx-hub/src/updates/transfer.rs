//! The hub as one end of a package's data connection: handing a package it
//! holds to a member (`Intent::Package`), and taking one a member fetched
//! for it (`Intent::Deliver`). The hub keeps such a connection alive as
//! either end of a relayed one does: something of its own at least every
//! heartbeat, and the member taken for gone once nothing of its own has come
//! for the silence; nothing it writes may take longer than the silence
//! either. Packages are handed to a few members at once
//! (`super::HANDED_AT_ONCE`); the others wait, kept alive meanwhile.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use fastctx_net::link::versions::Sized;
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, HEARTBEAT, Refusal, SILENCE};
use fastctx_types::update::{Held, Manifest};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::Attempt;
use crate::{Hub, Socket, refuse, send};

/// What the member says once it has every byte.
const DONE: &str = "\"done\"";

/// Writes `message`, for as long as its bytes keep going out: a member
/// that takes nothing of what it is sent for a silence is gone as much as
/// one that says nothing.
async fn put<S>(sink: &mut S, pulse: &fastctx_net::pulse::Pulse, message: WsMessage) -> Result<(), String>
where
    S: futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match pulse.writing(sink.send(message)).await {
        Ok(sent) => sent.map_err(|error| error.to_string()),
        Err(_) => Err(format!("the member took nothing for {} s", SILENCE.as_secs())),
    }
}

/// The next message from the member, or why none comes: nothing of one
/// came for a silence.
async fn heard<S>(stream: &mut S, pulse: &fastctx_net::pulse::Pulse) -> Result<Option<WsMessage>, String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            next = stream.next() => return next.transpose().map_err(|error| error.to_string()),
            () = tokio::time::sleep_until(heard + SILENCE) => match pulse.since(heard) {
                Some(at) => heard = at,
                None => return Err(format!("nothing came from the member for {} s", SILENCE.as_secs())),
            },
        }
    }
}

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
    let (cache, handing) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.cache.clone(), updates.handing.clone())
    };
    let kept = cache.path(&version, &target).ok().filter(|path| path.is_file());
    let held = Held { version: version.clone(), target: target.clone() };
    let placed = match &kept {
        Some(_) => None,
        None => {
            let hub = hub.clone();
            tokio::task::spawn_blocking(move || super::placed(&hub, &held)).await.ok().flatten()
        }
    };
    if kept.is_none() && placed.is_none() {
        let reason = format!("the hub does not hold {target} of {version}");
        refuse(&mut socket, Refusal::Broken { reason }).await;
        return;
    }
    let opened = async {
        let mut file = match (&kept, placed) {
            (Some(path), _) => tokio::fs::File::open(path).await?,
            (None, Some((file, _))) => tokio::fs::File::from_std(file),
            (None, None) => unreachable!("refused above"),
        };
        let size = file.metadata().await?.len();
        file.seek(std::io::SeekFrom::Start(from.min(size))).await?;
        Ok::<_, std::io::Error>((file, size))
    };
    let (mut file, size) = match opened.await {
        Ok(opened) => opened,
        Err(error) => {
            let reason = format!("reading {target} of {version}: {error}");
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
    };
    if tokio::time::timeout(SILENCE, send(&mut socket, &Down::Paired)).await.map_or(true, |sent| sent.is_err()) {
        return;
    }
    let pulse = crate::pulse_of(&socket);
    let (mut sink, mut stream) = socket.split();
    let (body_sent, sent) = tokio::sync::oneshot::channel();
    let receipt = async {
        let at = match sent.await {
            Ok(at) => at,
            Err(_) => return std::future::pending::<Result<(), String>>().await,
        };
        tokio::time::sleep_until(at + std::time::Duration::from_secs(120)).await;
        Err("the member did not acknowledge the complete package within 120 seconds".to_string())
    };
    let writer = async {
        // Its turn among the members being handed packages, said to be
        // alive while it waits.
        let place = handing.acquire();
        tokio::pin!(place);
        let _place = loop {
            tokio::select! {
                place = &mut place => break place.map_err(|error| error.to_string())?,
                () = tokio::time::sleep(HEARTBEAT) => put(&mut sink, &pulse, fastctx_net::pulse::ping(&pulse)).await?,
            }
        };
        let said = serde_json::to_string(&Sized { size }).expect("a size serializes");
        put(&mut sink, &pulse, WsMessage::Text(said.into())).await?;
        // As many bytes to a message as the member's line takes.
        let mut pace = fastctx_net::data::Pace::default();
        let mut buffer = vec![0; fastctx_net::data::CHUNK];
        loop {
            let read = file.read(&mut buffer[..pace.chunk()]).await.map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            let began = std::time::Instant::now();
            put(&mut sink, &pulse, WsMessage::Binary(buffer[..read].to_vec().into())).await?;
            pace.wrote(read, began.elapsed());
        }
        let _ = body_sent.send(tokio::time::Instant::now());
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            put(&mut sink, &pulse, fastctx_net::pulse::ping(&pulse)).await?;
        }
    };
    let reader = async {
        loop {
            match heard(&mut stream, &pulse).await? {
                Some(WsMessage::Text(text)) if text.as_str() == DONE => return Ok(()),
                Some(WsMessage::Close(_)) | None => return Err("the member left".to_string()),
                // What the member says it took keeps the package going to it;
                // one that is there and takes nothing has its place let go.
                Some(WsMessage::Ping(said)) => {
                    if let Some(took) = fastctx_net::pulse::said(&said) {
                        pulse.told(took);
                    }
                }
                Some(_) => {}
            }
        }
    };
    let ended: Result<(), String> = tokio::select! {
        ended = reader => ended,
        ended = writer => ended,
        ended = receipt => ended,
    };
    match ended {
        Ok(()) => hub.log.line(format_args!("{me} took {target} of {version} from the hub")),
        Err(why) => hub.log.line(format_args!("{me} did not take {target} of {version}: {why}")),
    }
}

/// Has a try over once its transfer is, however it ends.
struct Over<'a>(&'a Attempt);

impl Drop for Over<'_> {
    fn drop(&mut self) {
        self.0.over();
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
        updates
            .fetches
            .get(&id)
            .filter(|fetching| fetching.helper == me)
            .map(|fetching| (fetching.held.clone(), fetching.attempt.clone()))
    };
    let Some((held, attempt)) = asked else {
        let reason = format!("the hub asked {me} for no package numbered {id}");
        refuse(&mut socket, Refusal::Broken { reason }).await;
        return;
    };
    // One transfer a try, and none once the hub gave the try up.
    if !attempt.begin() {
        let reason = format!("the hub takes no more of request {id} from {me}");
        refuse(&mut socket, Refusal::Broken { reason }).await;
        return;
    }
    let outcome = {
        let _over = Over(&attempt);
        take(&hub, socket, &held, &attempt).await
    };
    if let Err(why) = &outcome {
        hub.log.line(format_args!("taking {} of {} from {me}: {why}", held.target, held.version));
    }
    if let Some(done) =
        hub.updates.lock().expect("never poisoned").fetches.get_mut(&id).and_then(|fetching| fetching.done.take())
    {
        let _ = done.send(outcome);
    }
}

async fn take(hub: &Arc<Hub>, mut socket: Socket, held: &Held, attempt: &Attempt) -> Result<(), String> {
    let (manifest, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(&held.version), updates.cache.clone())
    };
    let manifest = manifest.ok_or_else(|| format!("the hub no longer knows the release {}", held.version))?;
    let incoming = cache.attempt(&held.version, &held.target, attempt.id)?;
    tokio::time::timeout(SILENCE, send(&mut socket, &Down::Paired))
        .await
        .map_err(|_| format!("the member took nothing for {} s", SILENCE.as_secs()))??;
    let pulse = crate::pulse_of(&socket);
    let (sink, mut stream) = socket.split();
    let work = fastctx_net::data::ReceiptWork::default();
    let writer = super::receipt_writer::Writer::new(sink, pulse.clone(), work.clone());
    let received = async {
        let mut file =
            tokio::fs::File::create(&incoming).await.map_err(|error| format!("{}: {error}", incoming.display()))?;
        let mut size = None;
        let mut have = 0_u64;
        while size.is_none_or(|size| have < size) {
            match heard(&mut stream, &pulse).await? {
                Some(WsMessage::Text(text)) if size.is_none() => {
                    let said: Sized = serde_json::from_str(&text).map_err(|error| error.to_string())?;
                    size = Some(said.size);
                }
                Some(WsMessage::Binary(bytes)) if size.is_some() => {
                    let _writing = work.begin();
                    file.write_all(&bytes).await.map_err(|error| error.to_string())?;
                    have += bytes.len() as u64;
                    attempt.brought.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
                Some(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
                Some(WsMessage::Close(_)) | None => {
                    return Err("the member left before it sent it all".to_string());
                }
                Some(_) => return Err("the member sent something other than the package".to_string()),
            }
        }
        if have != size.unwrap_or_default() {
            return Err("the member sent more than it said".to_string());
        }
        let _settling = work.begin();
        file.sync_all().await.map_err(|error| error.to_string())
    };

    let completing = async {
        let received: Result<(), String> = tokio::select! {
            received = received => received,
            () = attempt.stopping() => Err("the hub gave this request up and asked another machine".to_string()),
        };
        let _settling = work.begin();
        let kept = match received {
            Ok(()) if attempt.keep() => {
                take_in(hub, &cache, &manifest, &held.target, &incoming, || attempt.commit()).await.map(|_| ())
            }
            Ok(()) => Err("the hub gave this request up and asked another machine".to_string()),
            Err(why) => Err(why),
        };
        super::cleanup::temporary(hub, &incoming).await;
        kept
    };
    let kept = completing.await;
    let word = match &kept {
        Ok(()) => DONE.to_string(),
        Err(why) => serde_json::to_string(&format!("the hub did not take it: {why}")).expect("text serializes"),
    };
    if let Err(why) = writer.finish(word).await {
        hub.log.line(format_args!("sending the package receipt: {why}"));
    }
    kept
}

/// Checks the package that came to `incoming` and keeps it, if `may_keep`
/// still says it may once it is checked, telling every machine the hub holds
/// it; its size.
pub(super) async fn take_in(
    hub: &Arc<Hub>,
    cache: &fastctx_update::Cache,
    manifest: &Manifest,
    target: &str,
    incoming: &Path,
    may_keep: impl FnOnce() -> bool,
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
    if !may_keep() {
        let _ = std::fs::remove_file(incoming);
        return Err("the hub gave this request up while it checked the package".to_string());
    }
    let (publishing, kept_manifest, kept_target, received) =
        (cache.clone(), manifest.clone(), target.to_string(), incoming.to_path_buf());
    let size = tokio::task::spawn_blocking(move || {
        publishing.keep_manifest(&kept_manifest)?;
        let path = publishing.keep(&kept_manifest.version, &kept_target, &received)?;
        std::fs::metadata(&path).map(|meta| meta.len()).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    hub.log.line(format_args!("holds {target} of {} now, {size} bytes", manifest.version));
    super::prune(hub);
    super::tell(hub, None);
    Ok(size)
}
