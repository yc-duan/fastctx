//! Frozen data-channel handshake shapes; no network connection is opened.
use fastctx_net::{
    sign::{Domain, PublicKey, Signature},
    sync_transport::NegotiatedTransport,
};
use fastctx_types::sync::{
    cursor::ByteCount,
    session::{SessionId, TransportLimits},
    transfer::Capabilities,
};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let capabilities = Capabilities {
        protocols: vec!["fastctx-sync/v1".into()],
        envelopes: vec!["plain/v1".into()],
        encodings: vec!["raw".into(), "zstd".into()],
        transports: vec!["asset-transfer/v1".into(), "future-transport".into()],
        receive_window: ByteCount(4096),
        max_page_bytes: 262144,
        ..Default::default()
    };
    let limits = TransportLimits {
        max_frame_bytes: 65536,
        max_control_bytes: 16384,
        max_metadata_bytes: 262144,
        max_active_transfers: 4,
        max_active_parts: 2,
    };
    let selected = NegotiatedTransport::select(&capabilities, &capabilities, &limits, &limits)?;
    let mut old = capabilities.clone();
    old.transports.clear();
    assert!(NegotiatedTransport::select(&capabilities, &old, &limits, &limits).is_err());
    old.transports = vec!["future-transport".into()];
    assert!(NegotiatedTransport::select(&capabilities, &old, &limits, &limits).is_err());
    let key = PublicKey::from_bytes([0; 32]);
    let proof = Signature::from_bytes([0; 64]);
    for domain in [Domain::Session, Domain::Peer] {
        assert!(key.verify(domain, b"wire-v1-placeholder", &proof).is_err());
    }
    let session = SessionId::from_bytes([41; 16]);
    let hub = selected.hub_intent(session.clone(), key, proof);
    let peer = selected.peer_intent(session, key, proof);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "proof_status":"invalid_placeholder_not_a_signature",
            "advertisement":capabilities,"selection":selected.selection(),"limits":selected.limits(),
            "hub_intent":hub,"direct_device_intent":peer,
            "no_or_unknown_transport_advertisement":"refused_before_new_intent",
            "service_activation":false
        }))?
    );
    Ok(())
}
