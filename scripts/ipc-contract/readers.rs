//! Compiled against unchanged type sources from pinned git revisions, one for each protocol.

use serde_json::{Value, json};
use std::io::{self, BufRead};

#[path = "COMPAT_PATH"]
mod compat;

fn decode(protocol: u64, value: Value, request: bool) -> Value {
    macro_rules! read {
        ($version:ident) => {
            $version::contract_frame(value, request)
        };
    }
    match protocol {
        5 => read!(v5),
        6 => read!(v6),
        7 => read!(v7),
        8 => read!(v8),
        9 => read!(v9),
        10 => read!(v10),
        _ => panic!("no frozen reader for {protocol}"),
    }
}

fn main() {
    for line in io::stdin().lock().lines() {
        let case: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let protocol = case["protocol"].as_u64().unwrap();
        let value = case["value"].clone();
        let answer = match case["mode"].as_str().unwrap() {
            "welcome" => {
                let current: fastctx_types::ipc::Welcome = serde_json::from_value(value).unwrap();
                let wire = serde_json::to_value(current).unwrap();
                macro_rules! welcome {
                    ($version:ident) => {
                        $version::contract_welcome(wire)
                    };
                }
                let read = match protocol {
                    5 => welcome!(v5),
                    6 => welcome!(v6),
                    7 => welcome!(v7),
                    8 => welcome!(v8),
                    9 => welcome!(v9),
                    10 => welcome!(v10),
                    _ => panic!("no frozen reader for {protocol}"),
                };
                json!({"read": read})
            }
            "request" => {
                let old = decode(protocol, value, true);
                let fastctx_types::ipc::ClientFrame::Call { call, .. } = serde_json::from_value(old).unwrap();
                let peer = serde_json::from_value(case["peer"].clone()).unwrap();
                let accepted =
                    protocol as u32 == fastctx_types::ipc::PROTOCOL || compat::enabled(protocol as u32, &peer);
                json!({"admitted": accepted && compat::call(protocol as u32, &peer, &call)})
            }
            "current_request" => {
                let fastctx_types::ipc::ClientFrame::Call { call, .. } = serde_json::from_value(value).unwrap();
                let peer = serde_json::from_value(case["peer"].clone()).unwrap();
                let accepted =
                    protocol as u32 == fastctx_types::ipc::PROTOCOL || compat::enabled(protocol as u32, &peer);
                json!({"admitted": accepted && compat::peer(&peer) && compat::call(protocol as u32, &peer, &call)})
            }
            "decode" => json!({"read": decode(protocol, value, false)}),
            "reply" => {
                let frame: fastctx_types::ipc::NodeFrame = serde_json::from_value(value).unwrap();
                let current = serde_json::to_value(&frame).unwrap();
                let fastctx_types::ipc::NodeFrame::Reply { id, mut reply } = frame else { panic!("expected reply") };
                if let Ok(reply) = &mut reply {
                    compat::reply(protocol as u32, reply);
                }
                let wire: Value = serde_json::from_slice(&compat::frame(protocol as u32, id, &reply)).unwrap();
                json!({"read": decode(protocol, wire.clone(), false), "wire": wire, "current": current})
            }
            other => panic!("unknown mode {other}"),
        };
        println!("{answer}");
    }
}
