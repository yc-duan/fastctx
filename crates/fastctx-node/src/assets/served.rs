//! This machine's end of what it keeps of the conversations it orders,
//! given to the machines that hold a copy (`direct`,
//! `fastctx_types::asset_peers`).
//!
//! One exchange after another on the data connection of the call that
//! brought it, until the asking machine lets the connection go. Nothing is
//! given without the hub's word, under the key this machine pinned for the
//! hub, that the asking device holds the scope until a time not yet past;
//! and only what this machine orders is given: frames of its own streams,
//! and objects those streams' frames refer to.

use super::Assets;
use super::direct::{CLOCKS_APART, machine_orders};
use super::session::blocking;
use crate::Node;
use fastctx_net::data::Stream;
use fastctx_net::link::Caller;
use fastctx_platform::readings::counter_of;
use fastctx_types::access::HubId;
use fastctx_types::asset_lines::{FRAME_BYTES, FRAMES, OBJECTS, RUN_BYTES};
use fastctx_types::asset_peers::{Ask, Head, Permit, SCOPES, Told};
use fastctx_types::assets::{self as wire};
use fastctx_types::ipc::Failure;
use fastctx_content::v2::Header;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// How far ahead of the hub's clock the end of its word may lie: one
/// further off was not made by a hub that gives its word for minutes.
const FAR_AHEAD: i64 = 600_000;
/// How long an ask waits for room while as many are given to at once as
/// are: less than the one asking waits for an answer.
const WAITED_FOR: Duration = Duration::from_secs(20);

/// Whether `permit` is the hub's word for the device `caller` proved it is,
/// standing now: checked once for all an ask names.
fn stands(assets: &Assets, hub: &HubId, caller: &Caller, permit: &Permit) -> Result<(), String> {
    if permit.granted.len() > SCOPES {
        return Err(format!("the hub's word names {SCOPES} scopes at most"));
    }
    let now = assets.direct.hub_now(hub);
    if now > permit.until.saturating_add(CLOCKS_APART) {
        return Err("the hub's word ran out".into());
    }
    if permit.until > now.saturating_add(FAR_AHEAD) {
        return Err("the hub's word ends further off than a hub gives it".into());
    }
    let Some((_, key)) = assets.current.lock().expect("sessions").get(hub).copied() else {
        return Err("this machine has no session with the hub to check its word by".into());
    };
    let statement = wire::permit_statement(caller.key.as_bytes(), permit.until, &permit.granted);
    if fastctx_net::sign::heard_word(&key, &statement, &permit.word.0).is_none() {
        return Err("the word is not the hub's".into());
    }
    Ok(())
}

/// Whether `permit` is the hub's word that the device `caller` proved it is
/// holds `scope`, standing now.
fn permitted(assets: &Assets, hub: &HubId, caller: &Caller, permit: &Permit, scope: &str) -> Result<(), String> {
    if !wire::of_conversation(scope) {
        return Err(format!("{scope} is no conversation's"));
    }
    if !permit.granted.iter().any(|granted| granted == scope) {
        return Err(format!("the hub's word does not name {scope}"));
    }
    stands(assets, hub, caller, permit)
}

/// Says why an ask is refused.
async fn refuse(stream: &mut Stream, why: String) -> Result<(), String> {
    stream.tell(&Told::Failed { why }).await
}

/// Where the streams this machine orders of `scopes` stand.
async fn heads(assets: &Assets, hub: &HubId, caller: &Caller, stream: &mut Stream, permit: Permit, scopes: Vec<String>) -> Result<(), String> {
    if scopes.len() > SCOPES {
        return refuse(stream, format!("where streams stand is asked of {SCOPES} scopes at a time at most")).await;
    }
    // The word is checked once; a word that does not stand is said so, and
    // the one asking takes another from the hub.
    if let Err(why) = stands(assets, hub, caller, &permit) {
        return refuse(stream, why).await;
    }
    let named: HashSet<&str> = permit.granted.iter().map(String::as_str).collect();
    let granted: BTreeSet<String> =
        scopes.into_iter().filter(|scope| wire::of_conversation(scope) && named.contains(scope.as_str())).collect();
    let device = assets.device();
    let found = blocking(&assets.library, move |library| {
        let mut heads = Vec::new();
        for scope in granted {
            let own: HashSet<String> = library.own_streams(&scope, &device)?.into_iter().collect();
            // What the hub once refused of a stream waits for the stream's
            // new epoch (`Library::waiting`): until it has one, the stream
            // stands, for those that hold a copy, where it was cut.
            let cut: HashMap<String, u64> =
                library.waiting(&scope)?.into_iter().filter(|(_, _, renewed)| !renewed).map(|(stream, cut, _)| (stream, cut)).collect();
            for (name, next) in library.streams(&scope)? {
                let next = cut.get(&name).map_or(next, |cut| next.min(*cut));
                if next > 0 && own.contains(&name) && machine_orders(&name) {
                    heads.push(Head { scope: scope.clone(), stream: name, next });
                }
            }
        }
        Ok(heads)
    })
    .await;
    match found {
        Ok(heads) => stream.tell(&Told::Heads { heads }).await,
        Err(why) => refuse(stream, why).await,
    }
}

/// Frames of a stream this machine orders, their bodies after them.
#[allow(clippy::too_many_arguments)]
async fn frames(
    assets: &Assets,
    hub: &HubId,
    caller: &Caller,
    stream: &mut Stream,
    permit: Permit,
    scope: String,
    name: String,
    from: u64,
    most: u32,
) -> Result<(), String> {
    if let Err(why) = permitted(assets, hub, caller, &permit, &scope) {
        return refuse(stream, why).await;
    }
    if !machine_orders(&name) {
        return refuse(stream, format!("the hub orders {name} itself")).await;
    }
    let device = assets.device();
    let (scope_text, name_text) = (scope.clone(), name.clone());
    let exported = blocking(&assets.library, move |library| {
        if library.orderer(&scope_text, &name_text)? != Some(device) {
            return Ok(None);
        }
        // Nothing from where the hub's refusal cut it, until its new epoch
        // is open: what stands there now is committed again in that epoch.
        // And nothing read while that changed: what was read may be of the
        // epoch before, cut away meanwhile; asked again, it is read anew.
        let waits = |library: &fastctx_content::v2::Library| {
            Ok::<_, fastctx_content::Error>(library.waiting(&scope_text)?.into_iter().find(|(stream, _, _)| *stream == name_text).map(|(_, cut, renewed)| (cut, renewed)))
        };
        let before = waits(library)?;
        let mut frames = library.export_within(&scope_text, &name_text, from, (most as usize).clamp(1, FRAMES), FRAME_BYTES)?;
        let after = waits(library)?;
        if before != after {
            frames.clear();
        }
        if let Some((cut, false)) = after {
            frames.retain(|frame| Header::decode(&frame.header).is_ok_and(|header| header.first + u64::from(header.count) <= cut));
        }
        Ok(Some(frames))
    })
    .await;
    let exported = match exported {
        Ok(Some(exported)) => exported,
        Ok(None) => return refuse(stream, format!("this machine does not order {name} of {scope}")).await,
        Err(why) => return refuse(stream, why).await,
    };
    let mut bodies = Vec::with_capacity(exported.iter().map(|frame| frame.body.len()).sum());
    let mut heads = Vec::with_capacity(exported.len());
    for frame in exported {
        heads.push(wire::Frame {
            header: frame.header.into(),
            signature: frame.signature.into(),
            body: Default::default(),
            apart: frame.body.len() as u64,
        });
        bodies.extend_from_slice(&frame.body);
    }
    stream.tell(&Told::Frames { frames: heads }).await?;
    // A frame's records are compressed as they are kept.
    stream.give(&bodies, true).await?;
    counter_of("sync.given", "frames").add(bodies.len() as u64);
    Ok(())
}

/// Objects the frames of streams this machine orders refer to, each whole.
async fn objects(
    assets: &Assets,
    hub: &HubId,
    caller: &Caller,
    stream: &mut Stream,
    permit: Permit,
    scope: String,
    wanted: Vec<wire::ObjectRef>,
) -> Result<(), String> {
    if wanted.is_empty() || wanted.len() > OBJECTS {
        return refuse(stream, format!("objects are asked for 1 to {OBJECTS} at a time")).await;
    }
    if let Err(why) = permitted(assets, hub, caller, &permit, &scope) {
        return refuse(stream, why).await;
    }
    let device = assets.device();
    for (index, object) in wanted.into_iter().enumerate() {
        let index = index as u32;
        let Some(hash) = object.hash.array() else {
            stream.tell(&Told::Object { index, given: false, why: "an object is named by its 32 bytes".into() }).await?;
            continue;
        };
        let (scope_text, size) = (scope.clone(), object.size);
        // Read here through a stream this machine orders, and whole here at
        // the length asked for.
        let held = blocking(&assets.library, move |library| {
            let mut mine = false;
            for name in library.streams_reading(&scope_text, &hash)? {
                if machine_orders(&name) && library.orderer(&scope_text, &name)? == Some(device) {
                    mine = true;
                    break;
                }
            }
            Ok(mine && library.size(&hash)? == Some(size))
        })
        .await;
        if !matches!(held, Ok(true)) {
            let why = "no frame this machine orders of the scope refers to it, or it is not whole here".to_string();
            stream.tell(&Told::Object { index, given: false, why }).await?;
            continue;
        }
        stream.tell(&Told::Object { index, given: true, why: String::new() }).await?;
        let mut offset = 0;
        while offset < size {
            let length = RUN_BYTES.min(size - offset) as u32;
            let bytes = blocking(&assets.library, move |library| library.read(&hash, offset, length)).await?;
            if bytes.is_empty() {
                return Err("an object kept here ended before its length".into());
            }
            stream.give(&bytes, false).await?;
            offset += bytes.len() as u64;
        }
        counter_of("sync.given", "objects").add(size);
    }
    Ok(())
}

/// Answers what `caller` asks of the streams kept here, on the data
/// connection of its call, for as long as it keeps the connection.
pub(crate) async fn serve(node: &Arc<Node>, hub: &HubId, caller: &Caller, mut stream: Stream) -> Result<(), Failure> {
    let Some(assets) = node.assets.as_ref() else { return Err(Failure::new("the asset library is unavailable")) };
    loop {
        // The connection let go, however it went: nothing is owed.
        let Ok(ask) = stream.told::<Ask>("what is asked of the streams kept here").await else { return Ok(()) };
        // Room is held while something is given, not while a line waits to
        // be asked on: as many given to at once as are, an ask waits its
        // turn a while, and is told to come again where none comes.
        let Ok(Ok(_giving)) = tokio::time::timeout(WAITED_FOR, assets.direct.giving.clone().acquire_owned()).await else {
            if stream.tell(&Told::Busy).await.is_err() {
                return Ok(());
            }
            continue;
        };
        let done = match ask {
            Ask::Heads { permit, scopes } => heads(assets, hub, caller, &mut stream, permit, scopes).await,
            Ask::Frames { permit, scope, stream: name, from, frames: most } => {
                frames(assets, hub, caller, &mut stream, permit, scope, name, from, most).await
            }
            Ask::Objects { permit, scope, objects: wanted } => objects(assets, hub, caller, &mut stream, permit, scope, wanted).await,
        };
        if let Err(why) = done {
            node.log.line(format_args!("giving {} what is kept here: {why}", caller.machine));
            return Ok(());
        }
    }
}
