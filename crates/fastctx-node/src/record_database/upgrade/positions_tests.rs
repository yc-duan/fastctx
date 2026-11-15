//! Tests of `positions`: what an earlier node's databases are given as they open.
use fastctx_storage::rusqlite::Connection;

/// A node's call facts kept by beta.17, at their third version, are
/// given the tables the calls' own versions are kept in as it opens.
#[test]
fn call_facts_kept_before_are_given_the_calls_versions() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE storage_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL) WITHOUT ROWID;
         INSERT INTO storage_versions VALUES ('node.run-facts', 3);",
    )
    .unwrap();
    db.execute_batch(crate::run_facts::SCHEMA).unwrap();
    db.execute_batch("DROP TABLE run_call_versions; DROP TABLE run_tracked;").unwrap();
    db.execute_batch("BEGIN").unwrap();
    super::runs(&db).unwrap();
    db.execute_batch("COMMIT").unwrap();
    for table in ["run_call_versions", "run_tracked"] {
        assert!(super::super::exists(&db, table).unwrap(), "{table}");
    }
    let version: u32 = db
        .query_row("SELECT version FROM storage_versions WHERE component='node.run-facts'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
}
