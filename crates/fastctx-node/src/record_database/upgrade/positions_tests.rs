//! Tests of `positions`: what an earlier node's databases are given as they open.
use fastctx_storage::rusqlite::Connection;

/// A node's call facts kept by beta.17, at their third version, are given
/// the table the calls' own versions are kept in, and the conversation's
/// row what is noted call by call, as it opens; the versions it had stay.
#[test]
fn call_facts_kept_before_are_given_the_calls_versions() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE storage_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL) WITHOUT ROWID;
         INSERT INTO storage_versions VALUES ('node.run-facts', 3);",
    )
    .unwrap();
    db.execute_batch(crate::run_facts::SCHEMA).unwrap();
    // beta.17's: no calls' versions, its conversations' versions alone.
    db.execute_batch(
        "DROP TABLE run_call_versions; DROP TABLE run_versions;
         CREATE TABLE run_versions (conversation TEXT PRIMARY KEY, version INTEGER NOT NULL) WITHOUT ROWID;
         INSERT INTO run_versions VALUES ('c', 5);",
    )
    .unwrap();
    db.execute_batch("BEGIN").unwrap();
    super::runs(&db).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert!(super::super::exists(&db, "run_call_versions").unwrap());
    for column in ["since", "last"] {
        assert!(super::super::column(&db, "run_versions", column).unwrap(), "{column}");
    }
    let kept: (i64, Option<i64>) = db
        .query_row("SELECT version, last FROM run_versions WHERE conversation='c'", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(kept, (5, None));
    let version: u32 = db
        .query_row("SELECT version FROM storage_versions WHERE component='node.run-facts'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
    // Opened again, nothing changes.
    db.execute_batch("BEGIN").unwrap();
    super::runs(&db).unwrap();
    db.execute_batch("COMMIT").unwrap();
}
