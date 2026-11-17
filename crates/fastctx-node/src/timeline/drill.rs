//! Read-only access to the unabridged reference history for the offline drill.

use crate::Node;
use fastctx_types::ResidentId;
use serde_json::json;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpListener;
use std::sync::Arc;

pub(super) fn start(node: &Arc<Node>) {
    let Some(path) = std::env::var_os("FASTCTX_TIMELINE_DRILL_PROBE") else { return };
    let node = Arc::downgrade(node);
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("drill probe socket");
        std::fs::write(path, listener.local_addr().expect("probe address").to_string()).expect("probe address file");
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(node) = node.upgrade() else { break };
            let mut request = String::new();
            if BufReader::new(&mut stream).read_line(&mut request).is_err() {
                continue;
            }
            let result = match request.trim().parse::<ResidentId>() {
                Ok(resident) => runtime.block_on(async {
                    let before = crate::held_history::stamp(&node, &resident).await?;
                    let read = crate::talk::history_quietly(&node, &resident).await?;
                    let after = crate::held_history::stamp(&node, &resident).await?;
                    Ok::<_, fastctx_types::ipc::Failure>(json!({
                        "history": read.history,
                        "unread": read.unread,
                        "unsettled": read.unsettled,
                        "unavailable": read.unavailable,
                        "stamp_before": before,
                        "stamp_after": after,
                    }))
                }),
                Err(error) => Err(fastctx_types::ipc::Failure::new(error.to_string())),
            };
            let response = match result {
                Ok(read) => read,
                Err(error) => json!({ "error": error }),
            };
            let mut writer = BufWriter::new(&mut stream);
            let _ = serde_json::to_writer(&mut writer, &response);
            let _ = writer.write_all(b"\n");
        }
    });
}
