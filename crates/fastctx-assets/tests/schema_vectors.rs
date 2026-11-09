//! The only persistent checks in this crate consume frozen public schema vectors.
use fastctx_assets::account::{Handler, Kind};
use fastctx_sync::registry::handlers::KindHandler;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Vectors {
    kind: String,
    schema: u16,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    body: Value,
    valid: bool,
}
#[test]
fn frozen_schema_vectors() {
    for source in [
        include_str!("../contracts/account.directory.v1.json"),
        include_str!("../contracts/account.profile.v1.json"),
        include_str!("../contracts/account.collaboration.v1.json"),
        include_str!("../contracts/account.settings.v1.json"),
        include_str!("../contracts/account.skill.v1.json"),
    ] {
        let vectors: Vectors = serde_json::from_str(source).unwrap();
        let kind = Kind::ALL.into_iter().find(|kind| kind.id() == vectors.kind).unwrap();
        for case in vectors.cases {
            let result = Handler(kind).validate(vectors.schema, &serde_json::to_vec(&case.body).unwrap());
            assert_eq!(result.is_ok(), case.valid, "{} / {}: {result:?}", vectors.kind, case.name);
        }
    }
}
