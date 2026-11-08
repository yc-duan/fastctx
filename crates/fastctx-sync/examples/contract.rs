//! Emits representative protocol messages from their Rust definitions.

use fastctx_sync::{
    cursor::{ByteCount, DurableAck, Revision, ScopeCursor, Sequence},
    identity::{FeedEpoch, HubId, ScopeId, ScopeKey},
    manifest::Encoding,
    transfer::{Capabilities, KindVersion, Message},
};

fn main() -> Result<(), serde_json::Error> {
    let scope = ScopeKey { hub: HubId::from_bytes([1; 8]), scope: ScopeId::from_bytes([2; 32]) };
    let cursor = ScopeCursor {
        scope: scope.clone(),
        epoch: FeedEpoch::from_bytes([3; 32]),
        acl_revision: Revision(1),
        applied: Sequence(9_007_199_254_740_993),
    };
    let messages = vec![
        Message::Hello {
            capabilities: Capabilities {
                protocol: 1,
                kinds: vec![KindVersion { kind: "conversation.metadata".into(), schema: 1 }],
                envelope: 1,
                encodings: vec![Encoding::Raw, Encoding::Zstd],
                receive_window: ByteCount(4 * 1024 * 1024),
                max_page_bytes: 256 * 1024,
            },
        },
        Message::Cursors { entries: vec![cursor.clone()], more: false },
        Message::Catalog { cursor: None, scope: scope.clone() },
        Message::Changes { after: cursor.clone(), max_bytes: 256 * 1024 },
        Message::Acknowledge { durable: DurableAck::Catalog { cursor } },
        Message::Revoked { scope, acl_revision: Revision(2) },
    ];
    println!("{}", serde_json::to_string_pretty(&messages)?);
    Ok(())
}
