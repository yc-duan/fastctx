//! What this phone keeps of each hub's assets, and how much (design
//! §28.39): the device's replica follows every link it has, while the
//! background service keeps the process, held as the tier the user chose —
//! recent, large files fetched as they are opened (the default), or
//! everything — kept beside the device as `replica.json`. The page reads
//! what is kept there first (`fastctx_client::device::copy_reads`).

use fastctx_client::World;
use fastctx_client::device::Device;
use fastctx_client::device::replica::Tier;
use std::path::PathBuf;

const FILE: &str = "replica.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    tier: String,
}

fn path(device: &Device) -> PathBuf {
    device.directory().join(FILE)
}

fn name(tier: Tier) -> &'static str {
    match tier {
        Tier::Light => "light",
        Tier::Whole => "whole",
    }
}

fn named(name: &str) -> Option<Tier> {
    match name {
        "light" => Some(Tier::Light),
        "whole" => Some(Tier::Whole),
        _ => None,
    }
}

/// The tier the user chose, light where none was chosen or it cannot be
/// read.
fn chosen(device: &Device) -> Tier {
    fastctx_platform::disk::read(path(device))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .and_then(|kept| named(&kept.tier))
        .unwrap_or(Tier::Light)
}

/// Starts keeping the copy, as the chosen tier holds, beside the device's
/// connection: it follows each link the device has from now on.
pub(super) async fn start(world: World) {
    let Some(device) = world.device().cloned() else { return };
    let tier = chosen(&device);
    let started = device.clone();
    match tokio::task::spawn_blocking(move || started.replicate(tier)).await {
        // A tier chosen while the copy was opening holds from now on
        // (codex-f92d833e phone #12).
        Ok(Ok(replica)) => replica.set_tier(chosen(&device)),
        Ok(Err(error)) => eprintln!("keeping this phone's copy of the World: {error}"),
        Err(error) => eprintln!("keeping this phone's copy of the World: {error}"),
    }
}

fn device() -> Result<std::sync::Arc<Device>, String> {
    super::world()?.device().cloned().ok_or_else(|| "this phone has no device identity".to_string())
}

/// How much this phone keeps: `light` or `whole`.
#[fastctx_gui_command::command]
pub(super) async fn copy_tier() -> Result<String, String> {
    let device = device()?;
    Ok(name(device.replica().map_or_else(|| chosen(&device), |replica| replica.tier())).to_string())
}

/// Keeps `tier` (`light` or `whole`) from now on: chosen for good, and the
/// copy held so at once.
#[fastctx_gui_command::command]
pub(super) async fn copy_tier_set(tier: String) -> Result<(), String> {
    let chosen = named(&tier).ok_or_else(|| format!("{tier} is not a way this phone keeps the World"))?;
    let device = device()?;
    let bytes = serde_json::to_vec(&Kept { tier: name(chosen).to_string() }).map_err(|error| error.to_string())?;
    fastctx_platform::publish(&path(&device), &bytes, fastctx_platform::Permissions::Private).map_err(|error| error.to_string())?;
    if let Some(replica) = device.replica() {
        replica.set_tier(chosen);
    }
    Ok(())
}
