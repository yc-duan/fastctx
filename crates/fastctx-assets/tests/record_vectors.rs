//! Frozen payload examples, independent of production service activation.
use fastctx_assets::records::Schema;
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
struct Vectors {
    schema: u16,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    family: String,
    name: String,
    body: Value,
    valid: bool,
}
#[test]
fn frozen_record_vectors() {
    let vectors: Vectors = serde_json::from_str(include_str!("../contracts/records.v1.json")).unwrap();
    assert_eq!(vectors.schema, 1);
    for case in vectors.cases {
        let schema = match case.family.as_str() {
            "usage" => Schema::Usage,
            "activity" => Schema::Activity,
            "read_position" => Schema::ReadPosition,
            "machine_settings" => Schema::MachineSettings,
            "audit" => Schema::Audit,
            "connection" => Schema::Connection,
            "preferences" => Schema::Preferences,
            "alert" => Schema::Alert,
            "log" => Schema::Log,
            "mcp_log" => Schema::McpLog,
            other => panic!("unknown frozen schema {other}"),
        };
        let result = schema.validate(&serde_json::to_vec(&case.body).unwrap());
        assert_eq!(result.is_ok(), case.valid, "{} / {}: {result:?}", case.family, case.name);
    }
}
