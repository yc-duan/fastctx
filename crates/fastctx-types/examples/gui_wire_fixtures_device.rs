//! The device shell's additional commands, serialized from their real types.
//! Kept separate from existing page fixtures while the device pages are wired.

use fastctx_types::device::{ConnectionMode, ConnectionState, DeviceHub, DeviceJoined, DeviceState};
use fastctx_types::world::{LinkState, Route};

fn main() {
    let name: fastctx_types::MachineName = "phone".parse().expect("fixture name");
    let hub: fastctx_types::HubName = "home".parse().expect("fixture hub");
    let account: fastctx_types::AccountName = "ada".parse().expect("fixture account");
    let local = ConnectionState { mode: ConnectionMode::Local, device: None };
    let unjoined =
        ConnectionState { mode: ConnectionMode::Device, device: Some(DeviceState { name: None, hubs: Vec::new() }) };
    let connected = ConnectionState {
        mode: ConnectionMode::Device,
        device: Some(DeviceState {
            name: Some(name.clone()),
            hubs: vec![DeviceHub {
                hub: hub.clone(),
                link: LinkState::Online {
                    since: "2026-10-04T12:00:00Z".parse().expect("fixture time"),
                    hub: "192.0.2.1:7443".into(),
                    through: None,
                    route: Route::System,
                    ms: None,
                },
                logins: vec![account.clone()],
            }],
        }),
    };
    let joined = DeviceJoined { hub, name, account };
    let value = serde_json::json!({ "local": local, "unjoined": unjoined, "connected": connected, "joined": joined });
    println!("{}", serde_json::to_string_pretty(&value).expect("fixtures serialize"));
}
