//! Compiled against unchanged type sources from pinned git revisions: one for each older protocol,
//! and the readers of the releases that shipped since, whose arms are written in from `SHIPPED`.

use serde_json::{Value, json};
use std::io::{self, BufRead};

#[path = "COMPAT_PATH"]
mod compat;

fn decode(reader: &str, value: Value, request: bool) -> Value {
    macro_rules! read {
        ($version:ident) => {
            $version::contract_frame(value, request)
        };
    }
    match reader {
        "v5" => read!(v5),
        "v6" => read!(v6),
        "v7" => read!(v7),
        "v8" => read!(v8),
        "v9" => read!(v9),
        "v10" => read!(v10),
        // SHIPPED_DECODE
        _ => panic!("no frozen reader {reader}"),
    }
}

fn main() {
    for line in io::stdin().lock().lines() {
        let case: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let protocol = case["protocol"].as_u64().unwrap();
        let reader = case["reader"].as_str().map(str::to_string).unwrap_or(format!("v{protocol}"));
        let reader = reader.as_str();
        let value = case["value"].clone();
        let answer = match case["mode"].as_str().unwrap() {
            "welcome" => {
                let current: fastctx_types::ipc::Welcome = serde_json::from_value(value).unwrap();
                let wire = serde_json::to_value(current).unwrap();
                macro_rules! welcome {
                    ($version:ident) => {
                        $version::contract_welcome(wire.clone())
                    };
                }
                let read = match reader {
                    "v5" => welcome!(v5),
                    "v6" => welcome!(v6),
                    "v7" => welcome!(v7),
                    "v8" => welcome!(v8),
                    "v9" => welcome!(v9),
                    "v10" => welcome!(v10),
                    // SHIPPED_WELCOME
                    _ => panic!("no frozen reader {reader}"),
                };
                json!({"read": read, "wire": wire})
            }
            "native_composer" => {
                // These variants require composer_documents negotiation. The
                // absent flag on an old Welcome never enables their encoding.
                let welcome: fastctx_types::ipc::Welcome = serde_json::from_value(json!({
                    "kind": "accepted", "machine": "desktop", "version": "1.0.0"
                })).unwrap();
                let enabled = match welcome {
                    fastctx_types::ipc::Welcome::Accepted { composer_documents, .. } => composer_documents,
                    _ => panic!("expected accepted welcome"),
                };
                let wire = match case["shape"].as_str().unwrap() {
                    "input" => serde_json::to_value(serde_json::from_value::<fastctx_types::composer::sync::Input>(value).unwrap()).unwrap(),
                    "frame" => serde_json::to_value(serde_json::from_value::<fastctx_types::composer::sync::Frame>(value).unwrap()).unwrap(),
                    "command" => serde_json::to_value(serde_json::from_value::<fastctx_types::watching::local::Command>(value).unwrap()).unwrap(),
                    "watched" => serde_json::to_value(serde_json::from_value::<fastctx_types::ipc::NodeFrame>(value).unwrap()).unwrap(),
                    _ => panic!("unknown native composer shape"),
                };
                json!({"unnegotiated": enabled, "read": wire})
            }
            "request" => {
                let old = decode(reader, value, true);
                let fastctx_types::ipc::ClientFrame::Call { call, .. } = serde_json::from_value(old).unwrap() else {
                    panic!("expected a call")
                };
                let peer = serde_json::from_value(case["peer"].clone()).unwrap();
                let accepted =
                    protocol as u32 == fastctx_types::ipc::PROTOCOL || compat::enabled(protocol as u32, &peer);
                json!({"admitted": accepted && compat::call(protocol as u32, &peer, &call)})
            }
            "current_request" => {
                let fastctx_types::ipc::ClientFrame::Call { call, .. } = serde_json::from_value(value).unwrap() else {
                    panic!("expected a call")
                };
                let peer = serde_json::from_value(case["peer"].clone()).unwrap();
                let accepted =
                    protocol as u32 == fastctx_types::ipc::PROTOCOL || compat::enabled(protocol as u32, &peer);
                json!({"admitted": accepted && compat::peer(&peer) && compat::call(protocol as u32, &peer, &call)})
            }
            "decode" | "watched" => json!({"read": decode(reader, value, false)}),
            "additive_request" => json!({"read": decode(reader, value, true)}),
            "world" => {
                let mut now: fastctx_types::world::WorldNow = serde_json::from_value(value).unwrap();
                compat::world(case["shapes"].as_u64().unwrap() as u32, &mut now);
                let frame = fastctx_types::ipc::NodeFrame::World { id: 37, world: Box::new(now) };
                json!({"read": decode(reader, serde_json::to_value(frame).unwrap(), false)})
            }
            "reply" => {
                let frame: fastctx_types::ipc::NodeFrame = serde_json::from_value(value).unwrap();
                let current = serde_json::to_value(&frame).unwrap();
                let fastctx_types::ipc::NodeFrame::Reply { id, mut reply } = frame else { panic!("expected reply") };
                if let Ok(reply) = &mut reply {
                    compat::reply(protocol as u32, reply);
                    // No historical reader says which notices it reads.
                    compat::notices(0, reply);
                }
                // Nor which reply shapes.
                compat::shapes(0, &mut reply);
                let wire: Value = serde_json::from_slice(&compat::frame(protocol as u32, id, &reply)).unwrap();
                json!({"read": decode(reader, wire.clone(), false), "wire": wire, "current": current})
            }
            other => panic!("unknown mode {other}"),
        };
        println!("{answer}");
    }
}
