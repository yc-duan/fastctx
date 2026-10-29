//! Compiled against unchanged type sources from pinned git revisions: one for each older protocol,
//! and the readers of the releases that shipped since.

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
        "v13_beta1" => read!(v13_beta1),
        "v13_beta2" => read!(v13_beta2),
        "v13_beta11" => read!(v13_beta11),
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
                        $version::contract_welcome(wire)
                    };
                }
                let read = match reader {
                    "v5" => welcome!(v5),
                    "v6" => welcome!(v6),
                    "v7" => welcome!(v7),
                    "v8" => welcome!(v8),
                    "v9" => welcome!(v9),
                    "v10" => welcome!(v10),
                    "v13_beta1" => welcome!(v13_beta1),
                    "v13_beta2" => welcome!(v13_beta2),
                    "v13_beta11" => welcome!(v13_beta11),
                    _ => panic!("no frozen reader {reader}"),
                };
                json!({"read": read})
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
            "decode" => json!({"read": decode(reader, value, false)}),
            "reply" => {
                let frame: fastctx_types::ipc::NodeFrame = serde_json::from_value(value).unwrap();
                let current = serde_json::to_value(&frame).unwrap();
                let fastctx_types::ipc::NodeFrame::Reply { id, mut reply } = frame else { panic!("expected reply") };
                if let Ok(reply) = &mut reply {
                    compat::reply(protocol as u32, reply);
                    // No historical reader says which notices it reads.
                    compat::notices(0, reply);
                }
                let wire: Value = serde_json::from_slice(&compat::frame(protocol as u32, id, &reply)).unwrap();
                json!({"read": decode(reader, wire.clone(), false), "wire": wire, "current": current})
            }
            other => panic!("unknown mode {other}"),
        };
        println!("{answer}");
    }
}
