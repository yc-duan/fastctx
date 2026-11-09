//! Frozen document-description contracts; body preparation is a separate gate.
use fastctx_assets::documents::{Handler, Kind};
use fastctx_sync::registry::handlers::KindHandler;
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
struct Vectors {
    schema: u16,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    kind: String,
    name: String,
    body: Value,
    valid: bool,
}
#[test]
fn frozen_document_vectors() {
    let vectors: Vectors = serde_json::from_str(include_str!("../contracts/documents.v1.json")).unwrap();
    for case in vectors.cases {
        let kind = Kind::ALL.into_iter().find(|kind| kind.id() == case.kind).unwrap();
        let result = Handler(kind).validate(vectors.schema, &serde_json::to_vec(&case.body).unwrap());
        assert_eq!(result.is_ok(), case.valid, "{} / {}: {result:?}", case.kind, case.name);
    }
}
