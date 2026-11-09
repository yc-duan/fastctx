//! Frozen document-description contracts; body preparation is a separate gate.
use fastctx_assets::{
    Error,
    documents::{self, Handler, Kind},
};
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
    schema: Option<u16>,
    #[serde(default)]
    stored: bool,
    #[serde(default)]
    trailing_spaces: usize,
    expected: Expected,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Expected {
    Ok,
    Invalid { contains: String },
}
#[test]
fn frozen_document_vectors() {
    let vectors: Vectors = serde_json::from_str(include_str!("../contracts/documents.v1.json")).unwrap();
    for case in vectors.cases {
        let kind = Kind::ALL.into_iter().find(|kind| kind.id() == case.kind).unwrap();
        let mut body = serde_json::to_vec(&case.body).unwrap();
        body.resize(body.len() + case.trailing_spaces, b' ');
        let result = if case.stored {
            documents::decode_stored(&body).map(|_| ())
        } else {
            Handler(kind).validate(case.schema.unwrap_or(vectors.schema), &body)
        };
        let matches = match (&case.expected, &result) {
            (Expected::Ok, Ok(())) => true,
            (Expected::Invalid { contains }, Err(Error::Invalid(message))) => message.contains(contains),
            _ => false,
        };
        assert!(matches, "{} / {}: expected {:?}, got {result:?}", case.kind, case.name, case.expected);
    }
}
