//! The device shell's additional commands, serialized from their real types.
//! Kept separate from existing page fixtures while the device pages are wired.

use fastctx_types::device::{ConnectionMode, ConnectionState, DeviceJoined, DeviceState};
use fastctx_types::world::{LinkState, Route};

fn main() {
    let name: fastctx_types::MachineName = "phone".parse().expect("fixture name");
    let world = "w-0123456789abcdef".to_string();
    let local = ConnectionState { mode: ConnectionMode::Local, device: None };
    let unjoined = ConnectionState {
        mode: ConnectionMode::Device,
        device: Some(DeviceState { name: None, world: None, link: LinkState::Alone }),
    };
    let connected = ConnectionState {
        mode: ConnectionMode::Device,
        device: Some(DeviceState {
            name: Some(name.clone()),
            world: Some(world.clone()),
            link: LinkState::Online {
                since: "2026-10-04T12:00:00Z".parse().expect("fixture time"),
                hub: "192.0.2.1:7443".into(),
                relay: None,
                route: Route::System,
            },
        }),
    };
    let joined = DeviceJoined { world, name };
    let value = serde_json::json!({ "local": local, "unjoined": unjoined, "connected": connected, "joined": joined });
    println!("{}", serde_json::to_string_pretty(&value).expect("fixtures serialize"));
}
